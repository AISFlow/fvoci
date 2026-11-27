//! Personal unit progress and course progress summaries.
//!
//! Personal progress writes intentionally emit no websocket broadcasts, no search index
//! updates, no webhooks, and no audit log entries so a learner's private per-unit state
//! never leaves the caller's own session.

use fvoci_model::{
    CompletionChange, CourseProgressSummary, JsonMap, UnitProgress, UnitProgressChange,
    course_review_intervals, suggested_next_interval_days,
};

use super::App;
use crate::error::AppError;
use crate::metrics::UnitCompletionAction;

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

    /// Creates or updates the caller's personal progress on a live unit of `course_id`,
    /// populating `suggested_next_interval_days` when completed.
    ///
    /// # Errors
    ///
    /// Returns 404 if `unit_id` is not a live unit of `course_id`, and the store's error
    /// if the write fails.
    pub async fn put_unit_progress(
        &self,
        course_id: &str,
        unit_id: &str,
        user_id: &str,
        change: &UnitProgressChange,
        course_properties: &JsonMap,
    ) -> Result<UnitProgress, AppError> {
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
