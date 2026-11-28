//! The chapters and units of a course: their outline, making one, moving one, and the
//! trash of them. A unit or chapter is a block, and is changed and deleted as one
//! (`App::patch_block`, `App::delete_block`).

use fvoci_model::{
    Block, BlockError, BlockType, Container, CourseLimits, DeletedUnit, NewUnit, Unit, UnitKind,
    UnitMove, UnitNode, UnitPatch,
};
use fvoci_store::ByAgent;

use super::blocks::{ChangeAction, ChangeNotification};
use super::list_page::ListPage;
use super::{App, Reached};
use crate::error::{AppError, OrMissing};
use crate::permissions::ContainerPermission;

/// What a member of the course who may not edit it is refused making a unit with.
pub const CREATE_UNIT_DENIED: &str = "access denied to create unit";
/// What a member of the course who may not edit it is refused changing a unit with.
pub const PATCH_UNIT_DENIED: &str = "access denied to patch unit";
/// What a member of the course who may not edit it is refused moving a unit with.
pub const MOVE_UNIT_DENIED: &str = "access denied to move unit";
/// What a member of the course who may not edit it is refused deleting a unit with.
pub const DELETE_UNIT_DENIED: &str = "access denied to delete unit";
/// What a member of the course who may not edit it is refused its trash with.
pub const UNIT_TRASH_DENIED: &str = "access denied to the trash of the course";
/// What a member of the course who may not edit it is refused bringing a unit back with.
pub const UNDELETE_UNIT_DENIED: &str = "access denied to undelete unit";

/// Checks that the caller may edit the course `on` reached, refusing in `denial`, and
/// that `unit`, which the request found by its identifier, is a unit or chapter of it.
fn editing_unit<'on>(
    on: &'on Reached,
    unit: &Block,
    denial: &str,
) -> Result<&'on Container, AppError> {
    let course = on.require(ContainerPermission::ManageCards, denial)?;
    if on.holds(unit) && UnitKind::of_block(&unit.block_type).is_some() {
        Ok(course)
    } else {
        Err(AppError::not_found("unit"))
    }
}

impl App {
    /// The unit or chapter of the course, its block read once: a block that is neither a
    /// unit nor a chapter of this course is not there, whatever else it is, and one in
    /// the trash is not there. Whether the asker may read the course is the caller's to
    /// have asked.
    ///
    /// # Errors
    ///
    /// Returns a not-found error for anything but a live unit or chapter of the course,
    /// and the store's error if the block cannot be read.
    pub async fn unit_of(&self, course_id: &str, unit_id: &str) -> Result<Block, AppError> {
        const WHAT: &str = "unit";
        let block = self.block(unit_id).await.or_missing(WHAT)?;
        if block.container_id == course_id && UnitKind::of_block(&block.block_type).is_some() {
            Ok(block)
        } else {
            Err(AppError::not_found(WHAT))
        }
    }

    /// The unit (not a chapter) of the course, its block read once: a chapter, a block of
    /// another course, anything else, or one in the trash is not there. For operations
    /// that only apply to units with bodies (comments, body versions, public unit reads).
    ///
    /// # Errors
    ///
    /// Returns a not-found error for anything but a live unit of the course, and the
    /// store's error if the block cannot be read.
    pub async fn content_unit_of(&self, course_id: &str, unit_id: &str) -> Result<Block, AppError> {
        let block = self.unit_of(course_id, unit_id).await?;
        if block.block_type != BlockType::Unit {
            return Err(AppError::not_found("unit"));
        }
        Ok(block)
    }

    /// The chapters and units of the course without their bodies, as their outline lists
    /// them.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the outline cannot be read.
    pub async fn unit_tree(&self, course_id: &str) -> Result<Vec<UnitNode>, AppError> {
        Ok(self.store().get_unit_tree(course_id).await?)
    }

    /// Makes a unit or chapter of the course `on` reached as `new` says, for an editor of
    /// it, held to the course limits, and tells websocket listeners and webhooks in the
    /// background of it and of any siblings numbered anew for it.
    ///
    /// # Errors
    ///
    /// Returns [`CREATE_UNIT_DENIED`] as [`Reached::require`] does, a not-found error if
    /// there is no such course, and a bad request if the request's shape is refused, a
    /// chapter names a parent, a unit's parent is not a chapter of the course, the
    /// sibling to go after is no sibling of that parent, the title may not be stored, or
    /// the course limits would be exceeded.
    pub async fn create_unit(&self, on: &Reached, new: &NewUnit) -> Result<Unit, AppError> {
        let course = on.require(ContainerPermission::ManageCards, CREATE_UNIT_DENIED)?;
        new.check_request()?;
        let (course_id, user_id) = (on.container_id(), on.user_id());
        let unit = self
            .write_tree(course, ChangeAction::Add, async || {
                self.store()
                    .create_unit(course_id, new, CourseLimits::DEFAULT, user_id)
                    .await
            })
            .await?;
        Ok(Unit::from(unit))
    }

    /// Changes the unit or chapter of the course `on` reached as `patch` says, for an
    /// editor of it, as the agent of `agent` if one writes: a chapter has no body.
    ///
    /// # Errors
    ///
    /// Returns [`PATCH_UNIT_DENIED`] as [`Reached::require`] does, a not-found error for
    /// a block that is no unit or chapter of the course, a bad request for a body of a
    /// chapter, a patch whose shape is refused or a body that names a file the course
    /// does not have, and what `App::patch_block_as` returns.
    pub async fn patch_unit(
        &self,
        on: &Reached,
        unit: &Block,
        patch: UnitPatch,
        agent: Option<ByAgent<'_>>,
    ) -> Result<Unit, AppError> {
        editing_unit(on, unit, PATCH_UNIT_DENIED)?;
        let kind = UnitKind::of_block(&unit.block_type).unwrap_or_default();
        if kind == UnitKind::Chapter && !patch.body.is_keep() {
            return Err(AppError::from(BlockError::BodyNotAllowed));
        }
        patch.check_request_for(kind)?;
        self.check_body_files(on.container_id(), patch.body.new_body())
            .await?;
        let patched = self
            .patch_block_as(
                &unit.id,
                &patch.into_block_patch(),
                on.user_id(),
                ChangeNotification::Send,
                agent,
            )
            .await?;
        Ok(Unit::from(patched))
    }

    /// Moves a unit or chapter of the course `on` reached where `to` says, for an editor
    /// of it, and tells websocket listeners and webhooks in the background of it and of
    /// any siblings numbered anew for it. Moving a chapter keeps its units below it;
    /// those units are not told of.
    ///
    /// # Errors
    ///
    /// Returns [`MOVE_UNIT_DENIED`] as [`Reached::require`] does, a not-found error if
    /// the course has no such unit or chapter, and a bad request if a chapter names a
    /// parent, a unit's parent is not a chapter of the course or is itself, or the
    /// sibling to go after is no other sibling there.
    pub async fn move_unit(
        &self,
        on: &Reached,
        unit: &Block,
        to: &UnitMove,
    ) -> Result<Unit, AppError> {
        let course = editing_unit(on, unit, MOVE_UNIT_DENIED)?;
        let (course_id, unit_id, user_id) = (on.container_id(), unit.id.as_str(), on.user_id());
        let unit = self
            .write_tree(course, ChangeAction::Update, async || {
                self.store()
                    .move_unit(course_id, unit_id, to, user_id)
                    .await
            })
            .await?;
        Ok(Unit::from(unit))
    }

    /// Deletes a unit or chapter of the course `on` reached, for an editor of it, as any
    /// block is deleted (`App::delete_block`).
    ///
    /// # Errors
    ///
    /// Returns [`DELETE_UNIT_DENIED`] as [`Reached::require`] does, a not-found error for
    /// a block that is no unit or chapter of the course, and what `App::delete_block`
    /// returns.
    pub async fn delete_unit(&self, on: &Reached, unit: &Block) -> Result<(), AppError> {
        editing_unit(on, unit, DELETE_UNIT_DENIED)?;
        self.delete_block(&unit.id, on.user_id(), ChangeNotification::Send)
            .await
    }

    /// The deleted units and chapters of the course `on` reached, for an editor of it,
    /// at the top of what went with them, one page of the list ([`ListPage`]).
    ///
    /// # Errors
    ///
    /// Returns [`UNIT_TRASH_DENIED`] as [`Reached::require`] does, and the store's error
    /// if the trash cannot be read.
    pub async fn deleted_units(
        &self,
        on: &Reached,
        page: ListPage,
    ) -> Result<Vec<DeletedUnit>, AppError> {
        on.require(ContainerPermission::ManageCards, UNIT_TRASH_DENIED)?;
        Ok(self
            .store()
            .get_deleted_units(on.container_id(), Some(page.paging()))
            .await?)
    }

    /// Brings a unit or chapter of the course `on` reached back from the trash, for an
    /// editor of it (with its units, for a chapter), held to the course limits, and
    /// tells websocket listeners and webhooks in the background of each block that came
    /// back and each sibling numbered anew.
    ///
    /// # Errors
    ///
    /// Returns [`UNDELETE_UNIT_DENIED`] as [`Reached::require`] does, a not-found error
    /// if the course's trash has no such top-level unit or chapter, and a bad request if
    /// the course limits would be exceeded.
    pub async fn undelete_unit(&self, on: &Reached, unit_id: &str) -> Result<Unit, AppError> {
        let course = on.require(ContainerPermission::ManageCards, UNDELETE_UNIT_DENIED)?;
        let (course_id, user_id) = (on.container_id(), on.user_id());
        let unit = self
            .write_tree(course, ChangeAction::Add, async || {
                self.store()
                    .undelete_unit(course_id, unit_id, CourseLimits::DEFAULT, user_id)
                    .await
            })
            .await
            .or_missing("unit")?;
        Ok(Unit::from(unit))
    }
}
