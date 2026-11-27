//! Personal spaced-repetition reviews, personal unit notes, and due-review notifications.
//!
//! Personal review and note writes intentionally emit no websocket broadcasts, no search
//! index updates, no webhooks, and no audit log entries so a learner's private study
//! state never leaks beyond their own session. Due-review notifications are delivered
//! solely to the owning learner's own notification inbox.

use fvoci_model::{
    CompletedUnitReview, NewUnitNote, NewUnitReview, ReviewStatusFilter, TimeZone, TodayReviewItem,
    UnitNote, UnitNoteChange, UnitNoteKind, UnitReview, day_at,
};

use super::App;
use crate::error::AppError;

impl App {
    /// Returns the calendar day (`YYYY-MM-DD`) at `now` in `user_id`'s configured time
    /// zone, falling back to UTC when the user leaves time zone on `auto`.
    ///
    /// # Errors
    ///
    /// Returns the store's error if settings cannot be read, or an internal error if
    /// `now` falls outside calendar years 1 to 9999.
    pub async fn user_today(&self, user_id: &str, now: i64) -> Result<String, AppError> {
        let settings = self.store().get_user_settings(user_id).await?;
        day_at(now, &settings.time_zone)
            .or_else(|| day_at(now, &TimeZone::default()))
            .ok_or_else(|| AppError::internal("current timestamp is outside supported years"))
    }

    /// Lists the caller's personal reviews on live units of `course_id`.
    ///
    /// # Errors
    ///
    /// Returns the store's error if reviews cannot be read.
    pub async fn list_my_course_reviews(
        &self,
        course_id: &str,
        user_id: &str,
        status: ReviewStatusFilter,
        unit_id: Option<&str>,
    ) -> Result<Vec<UnitReview>, AppError> {
        Ok(self
            .store()
            .list_my_course_reviews(course_id, user_id, status, unit_id)
            .await?)
    }

    /// Schedules a personal review on a live unit of `course_id`.
    ///
    /// # Errors
    ///
    /// Returns 404 if `unit_id` is not a live unit of `course_id`, and the store's error
    /// if the write fails.
    pub async fn schedule_unit_review(
        &self,
        course_id: &str,
        unit_id: &str,
        user_id: &str,
        new: &NewUnitReview,
        now: Option<i64>,
    ) -> Result<UnitReview, AppError> {
        Ok(self
            .store()
            .schedule_unit_review(course_id, unit_id, user_id, new, now)
            .await?)
    }

    /// Completes the caller's review `review_id` in `course_id`, computing the suggested
    /// next interval and optionally scheduling a follow-up review atomically.
    ///
    /// # Errors
    ///
    /// Returns 404 if `review_id` does not belong to `user_id` in `course_id` or its
    /// unit is no longer live, and the store's error if the write fails.
    pub async fn complete_unit_review(
        &self,
        course_id: &str,
        review_id: &str,
        user_id: &str,
        intervals: &[u32],
        next: Option<&NewUnitReview>,
        now: Option<i64>,
    ) -> Result<CompletedUnitReview, AppError> {
        Ok(self
            .store()
            .complete_unit_review(course_id, review_id, user_id, intervals, next, now)
            .await?)
    }

    /// Deletes the caller's review `review_id` in `course_id`.
    ///
    /// # Errors
    ///
    /// Returns 404 if `review_id` does not belong to `user_id` in `course_id`, and the
    /// store's error if the write fails.
    pub async fn delete_unit_review(
        &self,
        course_id: &str,
        review_id: &str,
        user_id: &str,
    ) -> Result<(), AppError> {
        Ok(self
            .store()
            .delete_unit_review(course_id, review_id, user_id)
            .await?)
    }

    /// Lists the caller's pending reviews due today or overdue in `workspace_id`.
    ///
    /// # Errors
    ///
    /// Returns the store's error if reviews cannot be read.
    pub async fn workspace_today_reviews(
        &self,
        workspace_id: &str,
        user_id: &str,
        today: &str,
    ) -> Result<Vec<TodayReviewItem>, AppError> {
        Ok(self
            .store()
            .list_workspace_today_reviews(workspace_id, user_id, today)
            .await?)
    }

    /// Lists the caller's personal notes on live unit `unit_id` of `course_id`.
    ///
    /// # Errors
    ///
    /// Returns 404 if `unit_id` is not a live unit of `course_id`, and the store's error
    /// if notes cannot be read.
    pub async fn list_my_unit_notes(
        &self,
        course_id: &str,
        unit_id: &str,
        user_id: &str,
        kind: Option<UnitNoteKind>,
    ) -> Result<Vec<UnitNote>, AppError> {
        Ok(self
            .store()
            .list_my_unit_notes(course_id, unit_id, user_id, kind)
            .await?)
    }

    /// Creates a personal note on live unit `unit_id` of `course_id`.
    ///
    /// # Errors
    ///
    /// Returns 404 if `unit_id` is not a live unit of `course_id`, and the store's error
    /// if the write fails.
    pub async fn create_unit_note(
        &self,
        course_id: &str,
        unit_id: &str,
        user_id: &str,
        new: &NewUnitNote,
    ) -> Result<UnitNote, AppError> {
        Ok(self
            .store()
            .create_unit_note(course_id, unit_id, user_id, new)
            .await?)
    }

    /// Updates the caller's personal note `note_id` in `course_id`.
    ///
    /// # Errors
    ///
    /// Returns 404 if `note_id` does not belong to `user_id` in `course_id` or its unit
    /// is no longer live, and the store's error if the write fails.
    pub async fn patch_unit_note(
        &self,
        course_id: &str,
        note_id: &str,
        user_id: &str,
        change: &UnitNoteChange,
    ) -> Result<UnitNote, AppError> {
        Ok(self
            .store()
            .patch_unit_note(course_id, note_id, user_id, change)
            .await?)
    }

    /// Deletes the caller's personal note `note_id` in `course_id`.
    ///
    /// # Errors
    ///
    /// Returns 404 if `note_id` does not belong to `user_id` in `course_id`, and the
    /// store's error if the write fails.
    pub async fn delete_unit_note(
        &self,
        course_id: &str,
        note_id: &str,
        user_id: &str,
    ) -> Result<(), AppError> {
        Ok(self
            .store()
            .delete_unit_note(course_id, note_id, user_id)
            .await?)
    }
}
