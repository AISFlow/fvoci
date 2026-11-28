//! Personal unit progress and course progress summaries.
//!
//! Personal progress writes intentionally emit no websocket broadcasts, no search index
//! updates, no webhooks, and no audit log entries so a learner's private per-unit state
//! never leaves the caller's own session.

use fvoci_model::{
    CompletionChange, Container, CourseProgressSummary, JsonMap, UnitProgress, UnitProgressChange,
    course_review_intervals, suggested_next_interval_days,
};

use super::{App, ContainerAccess, Reached};
use crate::error::AppError;
use crate::metrics::UnitCompletionAction;
use crate::permissions::ContainerPermission;

/// What a member of the workspace who may not read the course is refused recording
/// progress with.
pub const PROGRESS_DENIED: &str = "access denied to update unit progress";

/// Checks that the caller may record their own progress in the course `on` reached: a
/// reader of it who is enrolled in it ([`ContainerAccess::has_membership`]). Gives the
/// course.
///
/// # Errors
///
/// Returns [`PROGRESS_DENIED`] as [`Reached::require`] does, and 403 for a reader who has
/// not joined the course.
pub fn require_progress_recorder(on: &Reached) -> Result<&Container, AppError> {
    let course = on.require(ContainerPermission::View, PROGRESS_DENIED)?;
    if !on.access().is_some_and(ContainerAccess::has_membership) {
        return Err(AppError::forbidden(
            "join the course before recording progress",
        ));
    }
    Ok(course)
}

impl App {
    /// Returns the caller's personal progress rows for live units of `course_id`,
    /// populating `suggested_next_interval_days` for completed units.
    ///
    /// # Errors
    ///
    /// Returns the store's error if progress cannot be read.
    pub async fn my_course_progress(
        &self,
        course_id: &str,
        user_id: &str,
        course_properties: &JsonMap,
    ) -> Result<Vec<UnitProgress>, AppError> {
        let mut rows = self
            .store()
            .get_my_course_progress(course_id, user_id)
            .await?;
        if rows.iter().any(|row| row.completed_at.is_some()) {
            let intervals = course_review_intervals(course_properties);
            let latest = self
                .store()
                .latest_unit_review_intervals(course_id, user_id, None)
                .await?;
            for row in &mut rows {
                if row.completed_at.is_some() {
                    let current = latest.get(&row.unit_id).copied();
                    row.suggested_next_interval_days =
                        Some(suggested_next_interval_days(current, &intervals));
                }
            }
        }
        Ok(rows)
    }

    /// Creates or updates the caller's personal progress on a live unit of the course `on`
    /// reached, for who may record it there ([`require_progress_recorder`]), populating
    /// `suggested_next_interval_days` when completed.
    ///
    /// # Errors
    ///
    /// Returns what [`require_progress_recorder`] returns, 404 if `unit_id` is not a live
    /// unit of the course, and the store's error if the write fails.
    pub async fn put_unit_progress(
        &self,
        on: &Reached,
        unit_id: &str,
        change: &UnitProgressChange,
    ) -> Result<UnitProgress, AppError> {
        let course = require_progress_recorder(on)?;
        let (course_id, user_id) = (on.container_id(), on.user_id());
        let course_properties: &JsonMap = &course.properties;
        let mut progress = self
            .store()
            .upsert_unit_progress(course_id, unit_id, user_id, change)
            .await?;
        match change.completion {
            CompletionChange::Complete => {
                self.metrics()
                    .course_unit_completion(UnitCompletionAction::Completed);
            }
            CompletionChange::Incomplete => {
                self.metrics()
                    .course_unit_completion(UnitCompletionAction::Uncompleted);
            }
            CompletionChange::Keep => {}
        }
        if progress.completed_at.is_some() {
            let intervals = course_review_intervals(course_properties);
            let latest = self
                .store()
                .latest_unit_review_intervals(course_id, user_id, Some(unit_id))
                .await?;
            let current = latest.get(unit_id).copied();
            progress.suggested_next_interval_days =
                Some(suggested_next_interval_days(current, &intervals));
        }
        Ok(progress)
    }

    /// Returns the aggregate completion summary across active members of `course_id`.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the summary cannot be computed.
    pub async fn course_progress_summary(
        &self,
        course_id: &str,
    ) -> Result<CourseProgressSummary, AppError> {
        Ok(self.store().get_course_progress_summary(course_id).await?)
    }
}
