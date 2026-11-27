//! The chapters and units of a course: their outline, making one, moving one, and the
//! trash of them. A unit or chapter is a block, and is changed and deleted as one
//! ([`App::patch_block`], [`App::delete_block`]).

use fvoci_model::{
    Block, BlockType, CourseLimits, DeletedUnit, NewUnit, Unit, UnitKind, UnitMove, UnitNode,
};
use fvoci_store::{UnitRestore, UnitWrite};

use super::App;
use super::blocks::{ChangeAction, ChangeNotification, log_notification};
use super::list_page::ListPage;
use crate::error::AppError;

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
        match self.block(unit_id).await {
            Ok(block)
                if block.container_id == course_id
                    && UnitKind::of_block(&block.block_type).is_some() =>
            {
                Ok(block)
            }
            Ok(_) => Err(AppError::not_found(WHAT)),
            Err(error) if error.is_not_found() => Err(AppError::not_found(WHAT)),
            Err(error) => Err(error),
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

    /// Makes a unit or chapter of the course as `new` says, held to the course limits,
    /// and tells websocket listeners and webhooks in the background of it and of any
    /// siblings numbered anew for it.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if there is no such course, and a bad request if a
    /// chapter names a parent, a unit's parent is not a chapter of the course, the
    /// sibling to go after is no sibling of that parent, the title may not be stored, or
    /// the course limits would be exceeded.
    pub async fn create_unit(
        &self,
        course_id: &str,
        new: &NewUnit,
        user_id: &str,
    ) -> Result<Unit, AppError> {
        let course = self.store().get_container(course_id).await?;
        let written = {
            let _structure = self.collab().changing_structure(self, course_id).await;
            self.store()
                .create_unit(course_id, new, CourseLimits::DEFAULT, user_id)
                .await?
        };
        log_notification(
            ChangeNotification::Send,
            ChangeAction::Add,
            &written.unit.id,
        );
        Ok(self.announce_units(&course.workspace_id, written))
    }

    /// Moves a unit or chapter of the course where `to` says, and tells websocket
    /// listeners and webhooks in the background of it and of any siblings numbered anew
    /// for it. Moving a chapter keeps its units below it; those units are not told of.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the course has no such unit or chapter, and a bad
    /// request if a chapter names a parent, a unit's parent is not a chapter of the
    /// course or is itself, or the sibling to go after is no other sibling there.
    pub async fn move_unit(
        &self,
        course_id: &str,
        unit_id: &str,
        to: &UnitMove,
        user_id: &str,
    ) -> Result<Unit, AppError> {
        let course = self.store().get_container(course_id).await?;
        let written = {
            let _structure = self.collab().changing_structure(self, course_id).await;
            self.store()
                .move_unit(course_id, unit_id, to, user_id)
                .await?
        };
        log_notification(ChangeNotification::Send, ChangeAction::Update, unit_id);
        Ok(self.announce_units(&course.workspace_id, written))
    }

    /// The course's deleted units and chapters at the top of what went with them, one
    /// page of the list ([`ListPage`]).
    ///
    /// # Errors
    ///
    /// Returns the store's error if the trash cannot be read.
    pub async fn deleted_units(
        &self,
        course_id: &str,
        page: ListPage,
    ) -> Result<Vec<DeletedUnit>, AppError> {
        Ok(self
            .store()
            .get_deleted_units(course_id, Some(page.paging()))
            .await?)
    }

    /// Brings a unit or chapter of the course back from the trash (with its units, for a
    /// chapter), held to the course limits, and tells websocket listeners and webhooks
    /// in the background of each block that came back and each sibling numbered anew.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the course's trash has no such top-level unit or
    /// chapter, and a bad request if the course limits would be exceeded.
    pub async fn undelete_unit(
        &self,
        course_id: &str,
        unit_id: &str,
        user_id: &str,
    ) -> Result<Unit, AppError> {
        let course = self.store().get_container(course_id).await?;
        let restored = {
            let _structure = self.collab().changing_structure(self, course_id).await;
            self.store()
                .undelete_unit(course_id, unit_id, CourseLimits::DEFAULT, user_id)
                .await
        };
        let UnitRestore { units, renumbered } = restored.map_err(|error| {
            if error.is_not_found() {
                AppError::not_found("unit")
            } else {
                AppError::from(error)
            }
        })?;
        log_notification(ChangeNotification::Send, ChangeAction::Add, unit_id);
        let unit = units
            .first()
            .cloned()
            .map(Unit::from)
            .ok_or_else(|| AppError::not_found("unit"))?;
        let mut told = renumbered;
        told.extend(units);
        self.announce_course_blocks(&course.workspace_id, told);
        Ok(unit)
    }

    /// Tells websocket listeners and webhooks in the background of the blocks written,
    /// the siblings numbered anew first, and returns the unit or chapter the write was
    /// about.
    fn announce_units(&self, workspace_id: &str, written: UnitWrite) -> Unit {
        let UnitWrite { unit, renumbered } = written;
        let mut told: Vec<Block> = renumbered;
        told.push(unit.clone());
        self.announce_course_blocks(workspace_id, told);
        Unit::from(unit)
    }

    /// Tells websocket listeners and webhooks in the background of the blocks, in turn.
    fn announce_course_blocks(&self, workspace_id: &str, told: Vec<Block>) {
        let app = self.clone();
        let workspace_id = workspace_id.to_owned();
        self.spawn_background(async move {
            for block in &told {
                app.ws().broadcast_block_change(&workspace_id, block).await;
                app.tell_webhooks(block);
            }
        });
    }
}
