//! The chapters and units of a course: listing their outline, making one at its place
//! among its siblings, moving one within its course, and the trash of them.
//!
//! Chapters ([`BlockType::Chapter`]) sit at the top of the course with no body and no
//! parent; units ([`BlockType::Unit`]) sit at the top or below a chapter. Ordering,
//! sibling renumbering, and trash restoration share [`PageTree`], [`pages::write_place`],
//! and [`page_trash`] with wikis.

use std::collections::HashSet;

use fvoci_model::{
    Block, BlockError, BlockType, ContainerKind, CourseLimits, DeletedUnit, NewUnit, PageTree,
    UnitKind, UnitMove, UnitNode, new_id, sort_units,
};

use crate::blocks::Paging;
use crate::db::{Id, Scope};
use crate::page_trash::{self, place_taken, push_block_types_in_trash};
use crate::pages::{self, write_place};
use crate::trash::{block_from_json, block_not_in_trash, take_blocks_out_of_trash};
use crate::{Store, StoreError, blocks, containers, page_places};

/// The block types that make up a course's outline.
const COURSE_TYPES: [BlockType; 2] = [BlockType::Unit, BlockType::Chapter];

/// What a write of a course outline changed: the unit or chapter it was about, and any
/// siblings whose orders were made anew to leave it room.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitWrite {
    /// The unit or chapter made or moved, as it is stored.
    pub unit: Block,
    /// The siblings numbered anew, as they are stored, in the order they are listed in.
    pub renumbered: Vec<Block>,
}

/// What bringing a unit or chapter back from the trash changed: the blocks that came
/// back (the asked-for block first, a chapter before its units) and any siblings
/// numbered anew to leave it room.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitRestore {
    /// The units and chapters brought back, as they are stored.
    pub units: Vec<Block>,
    /// The siblings numbered anew, as they are stored.
    pub renumbered: Vec<Block>,
}

/// A course's live outline read under its container lock: the tree for sibling ordering
/// and placement, plus which nodes are chapters.
struct CourseOutline {
    tree: PageTree,
    chapters: HashSet<String>,
}

impl CourseOutline {
    fn chapter_count(&self) -> usize {
        self.chapters.len()
    }

    fn unit_count(&self) -> usize {
        self.tree.len().saturating_sub(self.chapters.len())
    }

    fn is_chapter(&self, id: &str) -> bool {
        self.chapters.contains(id)
    }

    fn check_limits(
        &self,
        add_units: usize,
        add_chapters: usize,
        limits: CourseLimits,
    ) -> Result<(), BlockError> {
        let max_chapters = usize::try_from(limits.chapters).unwrap_or(usize::MAX);
        if add_chapters > 0 && self.chapter_count().saturating_add(add_chapters) > max_chapters {
            return Err(BlockError::TooManyChapters {
                limit: limits.chapters,
            });
        }
        let max_units = usize::try_from(limits.units).unwrap_or(usize::MAX);
        if add_units > 0 && self.unit_count().saturating_add(add_units) > max_units {
            return Err(BlockError::TooManyUnits {
                limit: limits.units,
            });
        }
        Ok(())
    }

    fn check_parent(
        &self,
        kind: UnitKind,
        parent_id: &str,
        moving: Option<&str>,
    ) -> Result<(), BlockError> {
        match kind {
            UnitKind::Chapter => {
                if !parent_id.is_empty() {
                    return Err(BlockError::ChapterBelowBlock);
                }
            }
            UnitKind::Unit => {
                if parent_id.is_empty() {
                    return Ok(());
                }
                if moving == Some(parent_id) {
                    return Err(BlockError::ParentBelowBlock);
                }
                if !self.tree.contains(parent_id) {
                    return Err(BlockError::ParentNotOnContainer);
                }
                if !self.is_chapter(parent_id) {
                    return Err(BlockError::UnitParentNotChapter);
                }
            }
        }
        Ok(())
    }
}

impl Store {
    /// The chapters and units of a course without their bodies, in the order of
    /// [`sort_units`]. A container that is no course has none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_unit_tree(&self, course_id: &str) -> Result<Vec<UnitNode>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT id, parent_id, type, title, fields, created_at, updated_at FROM blocks \
                 WHERE container_id = ",
            );
            sql.bind(Id(course_id))
                .push(" AND type IN (")
                .bind_list(COURSE_TYPES.iter().copied().map(BlockType::as_str))
                .push(")");
            let rows = scope.fetch_all(sql).await?;
            let mut units = Vec::with_capacity(rows.len());
            for row in &rows {
                units.push(UnitNode::from(Block {
                    id: row.id(0)?,
                    parent_id: row.id(1)?,
                    block_type: crate::blocks::block_type_column(row, 2, "type")?,
                    title: row.text(3)?,
                    fields: row.json_map(4, "fields")?,
                    created_at: row.int(5)?,
                    updated_at: row.int(6)?,
                    ..Block::default()
                }));
            }
            sort_units(&mut units);
            Ok(units)
        })
        .await
    }

    /// Makes a unit or chapter of `course_id` as `new` says, held to `limits`, and
    /// returns it with any siblings numbered anew to leave it room. The course is locked
    /// before reading the outline so concurrent creations take distinct places.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such course, and
    /// [`StoreError::InvalidBlock`] if the container is no course, a chapter names a
    /// parent, a unit's parent is not a chapter of the course, `after` is no sibling of
    /// that parent, the title may not be stored, or `limits` would be exceeded.
    pub async fn create_unit(
        &self,
        course_id: &str,
        new: &NewUnit,
        limits: CourseLimits,
        user_id: &str,
    ) -> Result<UnitWrite, StoreError> {
        self.transaction(async |scope| {
            let Some(kind) = containers::lock_container_of_kind(scope, course_id).await? else {
                return Err(StoreError::not_found(format_args!("board ID={course_id}")));
            };
            let mut unit = Block {
                id: new_id(),
                container_id: course_id.to_owned(),
                block_type: new.kind.block_type(),
                title: new.title.clone(),
                fields: new.to_fields(),
                ..Block::default()
            };
            unit.parent_id.clone_from(&new.parent_id);
            let invalid = |source| StoreError::InvalidBlock {
                id: unit.id.clone(),
                source,
            };
            unit.validate().map_err(invalid)?;
            if kind != ContainerKind::Course {
                page_places::check(&unit, kind, None)?;
            }
            let outline = course_outline(scope, course_id).await?;
            outline
                .check_parent(new.kind, &unit.parent_id, None)
                .map_err(invalid)?;
            outline
                .check_limits(
                    usize::from(new.kind == UnitKind::Unit),
                    usize::from(new.kind == UnitKind::Chapter),
                    limits,
                )
                .map_err(invalid)?;
            let place = outline
                .tree
                .place(&unit.parent_id, new.after.as_deref(), "")
                .map_err(invalid)?;
            let renumbered = write_place(scope, &mut unit, place).await?;
            blocks::insert_block(scope, &mut unit, user_id).await?;
            Ok(UnitWrite { unit, renumbered })
        })
        .await
    }

    /// Moves a unit or chapter of `course_id` to the place `to` names, returning it with
    /// any siblings numbered anew. Moving a chapter keeps its child units below it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the course has no such unit or chapter, and
    /// [`StoreError::InvalidBlock`] if a chapter names a parent, a unit's parent is not a
    /// chapter of the course or is itself, or `after` is no other sibling of that parent.
    pub async fn move_unit(
        &self,
        course_id: &str,
        unit_id: &str,
        to: &UnitMove,
        user_id: &str,
    ) -> Result<UnitWrite, StoreError> {
        self.transaction(async |scope| {
            let not_found = || StoreError::not_found(format_args!("unit ID={unit_id}"));
            if containers::lock_container_of_kind(scope, course_id).await?
                != Some(ContainerKind::Course)
            {
                return Err(not_found());
            }
            let Some((mut unit, kind)) =
                blocks::find_block(scope, unit_id).await?.and_then(|block| {
                    let kind = UnitKind::of_block(&block.block_type)?;
                    (block.container_id == course_id).then_some((block, kind))
                })
            else {
                return Err(not_found());
            };
            let invalid = |source| StoreError::InvalidBlock {
                id: unit_id.to_owned(),
                source,
            };
            let parent_id = to.parent_id.as_str();
            let outline = course_outline(scope, course_id).await?;
            outline
                .check_parent(kind, parent_id, Some(unit_id))
                .map_err(invalid)?;
            if unit.parent_id == parent_id
                && outline
                    .tree
                    .is_at(unit_id, parent_id, to.after.as_deref())
                    .map_err(invalid)?
            {
                return Ok(UnitWrite {
                    unit,
                    renumbered: Vec::new(),
                });
            }
            let place = outline
                .tree
                .place(parent_id, to.after.as_deref(), unit_id)
                .map_err(invalid)?;
            parent_id.clone_into(&mut unit.parent_id);
            let renumbered = write_place(scope, &mut unit, place).await?;
            blocks::insert_block(scope, &mut unit, user_id).await?;
            Ok(UnitWrite { unit, renumbered })
        })
        .await
    }

    /// The units and chapters of `course_id` in the trash at the top of what was deleted
    /// with them, the last deleted first and then by identifier. A unit that went to the
    /// trash with its chapter comes back with that chapter and is not listed separately.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_deleted_units(
        &self,
        course_id: &str,
        paging: Option<Paging>,
    ) -> Result<Vec<DeletedUnit>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT top.block, top.deleted_by, top.deleted_at, \
                 (SELECT COUNT(*) FROM deleted_blocks went \
                 WHERE went.deletion_id = top.deletion_id AND ",
            );
            push_block_types_in_trash(&mut sql, "went.block_type", &COURSE_TYPES);
            sql.push(") FROM deleted_blocks top WHERE top.container_id = ")
                .bind(Id(course_id))
                .push(" AND ");
            push_block_types_in_trash(&mut sql, "top.block_type", &COURSE_TYPES);
            sql.push(
                " AND NOT EXISTS (SELECT 1 FROM deleted_blocks above \
                 WHERE above.deletion_id = top.deletion_id AND above.id = top.parent_id) \
                 ORDER BY top.deleted_at DESC, top.id",
            );
            sql.push_page(paging);
            let mut deleted = Vec::new();
            for row in &scope.fetch_all(sql).await? {
                let block = match block_from_json(row, 0) {
                    Ok(block) => block,
                    Err(StoreError::Corrupt { column, reason }) => {
                        tracing::error!(
                            course_id,
                            column,
                            %reason,
                            "skipping corrupt deleted unit in trash listing"
                        );
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let went = u32::try_from(row.int(3)?).unwrap_or(u32::MAX);
                let node = UnitNode::from(block);
                deleted.push(DeletedUnit {
                    id: node.id,
                    kind: node.kind,
                    title: node.title,
                    parent_id: node.parent_id,
                    deleted_at: row.int(2)?,
                    deleted_by: row.id(1)?,
                    below: went.saturating_sub(1),
                });
            }
            Ok(deleted)
        })
        .await
    }

    /// Brings a unit or chapter of `course_id` back from the trash (and, for a chapter,
    /// the units deleted along with it), held to `limits`. A unit whose chapter is gone
    /// comes back last at the top of the course.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the course's trash has no such top-level unit
    /// or chapter, and [`StoreError::InvalidBlock`] if `limits` would be exceeded.
    pub async fn undelete_unit(
        &self,
        course_id: &str,
        unit_id: &str,
        limits: CourseLimits,
        user_id: &str,
    ) -> Result<UnitRestore, StoreError> {
        self.transaction(async |scope| {
            if containers::lock_container_of_kind(scope, course_id).await?
                != Some(ContainerKind::Course)
            {
                return Err(block_not_in_trash());
            }
            let deletion =
                page_trash::deletion_of(scope, course_id, unit_id, &COURSE_TYPES).await?;
            undelete(scope, course_id, unit_id, deletion, limits, user_id).await
        })
        .await
    }
}

/// Reads the live outline of `course_id` under its container lock.
async fn course_outline(
    scope: &mut Scope<'_>,
    course_id: &str,
) -> Result<CourseOutline, StoreError> {
    let mut sql = scope.sql("SELECT id, parent_id, type, fields FROM blocks WHERE container_id = ");
    sql.bind(Id(course_id))
        .push(" AND type IN (")
        .bind_list(COURSE_TYPES.iter().copied().map(BlockType::as_str))
        .push(")");
    let rows = scope.fetch_all(sql).await?;
    let mut nodes = Vec::with_capacity(rows.len());
    let mut chapters = HashSet::new();
    for row in &rows {
        let id = row.id(0)?;
        let parent_id = row.id(1)?;
        let block_type = crate::blocks::block_type_column(row, 2, "type")?;
        let fields = row.json_map(3, "fields")?;
        if block_type == BlockType::Chapter {
            chapters.insert(id.clone());
        }
        nodes.push(UnitNode::from(Block {
            id,
            parent_id,
            block_type,
            fields,
            ..Block::default()
        }));
    }
    Ok(CourseOutline {
        tree: nodes.into_iter().collect(),
        chapters,
    })
}

/// Restores a deleted unit or chapter (with its units) under the course's lock.
async fn undelete(
    scope: &mut Scope<'_>,
    course_id: &str,
    unit_id: &str,
    deletion: page_trash::Deletion,
    limits: CourseLimits,
    user_id: &str,
) -> Result<UnitRestore, StoreError> {
    let invalid = |source| StoreError::InvalidBlock {
        id: unit_id.to_owned(),
        source,
    };
    let outline = course_outline(scope, course_id).await?;
    let adding_chapters = deletion
        .blocks
        .iter()
        .filter(|block| block.block_type == BlockType::Chapter)
        .count();
    let adding_units = deletion
        .blocks
        .iter()
        .filter(|block| block.block_type == BlockType::Unit)
        .count();
    outline
        .check_limits(adding_units, adding_chapters, limits)
        .map_err(invalid)?;

    let coming: PageTree = deletion
        .blocks
        .iter()
        .filter(|block| UnitKind::of_block(&block.block_type).is_some())
        .cloned()
        .map(UnitNode::from)
        .collect();
    let mut tops: Vec<Block> = deletion
        .blocks
        .into_iter()
        .filter(|block| !coming.contains(&block.parent_id))
        .collect();
    let mut renumbered = Vec::new();
    for top in &mut tops {
        let parent_gone = if top.block_type == BlockType::Chapter {
            !top.parent_id.is_empty()
        } else {
            !top.parent_id.is_empty() && !outline.is_chapter(&top.parent_id)
        };
        if parent_gone {
            top.parent_id.clear();
        }
        let place = if parent_gone {
            Some(outline.tree.place("", None, "").map_err(invalid)?)
        } else {
            place_taken(&outline.tree, top).map_err(invalid)?
        };
        if let Some(place) = place {
            renumbered.extend(pages::write_place(scope, top, place).await?);
        }
    }
    let now = scope.now_millis();
    let mut units = take_blocks_out_of_trash(scope, &deletion.id, user_id, now, |block| {
        if let Some(top) = tops.iter().find(|top| top.id == block.id) {
            block.parent_id.clone_from(&top.parent_id);
            block.fields.clone_from(&top.fields);
        }
    })
    .await?;
    if !units.iter().any(|block| block.id == unit_id) {
        return Err(block_not_in_trash());
    }
    // The asked-for unit or chapter first, then parents before what is below them: a
    // comment is one below its unit.
    units.sort_by_key(|block| {
        let depth = if block.block_type == BlockType::Comment {
            coming.depth(&block.parent_id) + 1
        } else {
            coming.depth(&block.id)
        };
        (block.id != unit_id, depth)
    });
    Ok(UnitRestore { units, renumbered })
}
