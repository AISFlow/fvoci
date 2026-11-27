//! Course schedules, shared/personal redistribution, personal planned-date overrides,
//! and workspace today/overdue aggregations.
//!
//! Shared schedule updates that reassign `Block.fields.scheduledDate` announce modified
//! unit blocks one at a time in the background (`blockChanged` websocket broadcast and
//! configured webhooks); personal planned-date updates and personal redistributions
//! intentionally emit no websocket broadcasts, no search index updates, no webhooks, and
//! no audit log entries.

use fvoci_model::{
    CourseScheduleView, DiaryDate, RedistributeScheduleRequest, UnitProgress,
    ValidatedCourseSchedule, WorkspaceLearningToday, build_course_schedule_view,
    today_in_time_zone,
};

use super::App;
use crate::error::AppError;
use crate::metrics::ScheduleRedistributionScope;

/// Whether the caller holds an enrolled membership on the course whose schedule is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CourseEnrollment {
    /// Caller is an enrolled member (or personal-space owner): include their own
    /// `unit_progress` rows (`myPlannedDate`, `completed`).
    Enrolled,
    /// Caller is an unjoined reader of an open course: omit personal progress rows.
    Unjoined,
}

impl App {
    /// Returns today's calendar date in the calling user's configured time zone (UTC
    /// when `timeZone` is `auto`).
    ///
    /// # Errors
    ///
    /// Returns the store's error if the user's settings cannot be read.
    pub async fn caller_today(&self, user_id: &str) -> Result<DiaryDate, AppError> {
        let settings = self.user_settings(user_id).await?;
        Ok(today_in_time_zone(
            self.schedule_now_millis(),
            &settings.time_zone,
        ))
    }

    /// Returns the course's shared schedule view for `user_id` using `user_id`'s time
    /// zone. Never modifies any stored dates.
    ///
    /// # Errors
    ///
    /// Returns the store's error if settings, schedule, units, or progress cannot be
    /// read.
    pub async fn course_schedule_view(
        &self,
        course_id: &str,
        user_id: &str,
        enrollment: CourseEnrollment,
    ) -> Result<CourseScheduleView, AppError> {
        let today = self.caller_today(user_id).await?;
        self.course_schedule_view_at(course_id, user_id, enrollment, today)
            .await
    }

    async fn course_schedule_view_at(
        &self,
        course_id: &str,
        user_id: &str,
        enrollment: CourseEnrollment,
        today: DiaryDate,
    ) -> Result<CourseScheduleView, AppError> {
        let schedule = self.store().get_course_schedule(course_id).await?;
        let nodes = self.store().get_unit_tree(course_id).await?;
        let progress = match enrollment {
            CourseEnrollment::Enrolled => {
                self.store()
                    .get_my_course_progress(course_id, user_id)
                    .await?
            }
            CourseEnrollment::Unjoined => Vec::new(),
        };
        Ok(build_course_schedule_view(
            schedule, &nodes, &progress, today,
        ))
    }

    /// Saves the shared schedule of `course_id`, optionally auto-assigning every live
    /// unit's shared `scheduledDate`, announces changed blocks in the background, and
    /// returns the updated schedule view.
    ///
    /// # Errors
    ///
    /// Returns 404 if `course_id` is not a live course, and the store's error on failure.
    pub async fn put_course_schedule(
        &self,
        course_id: &str,
        schedule: &ValidatedCourseSchedule,
        user_id: &str,
        enrollment: CourseEnrollment,
    ) -> Result<CourseScheduleView, AppError> {
        let course = self.store().get_container(course_id).await?;
        let updated_ids = {
            let _structure = self.collab().changing_structure(self, course_id).await;
            let (_, ids) = self
                .store()
                .upsert_course_schedule(course_id, schedule, user_id)
                .await?;
            ids
        };
        if schedule.auto_assign {
            self.metrics()
                .course_schedule_redistributed(ScheduleRedistributionScope::SharedAuto);
        }
        if !updated_ids.is_empty() {
            self.announce_blocks(&course.workspace_id, updated_ids);
        }
        self.course_schedule_view(course_id, user_id, enrollment)
            .await
    }

    /// Deletes the saved shared schedule of `course_id` (idempotent when none exists).
    ///
    /// # Errors
    ///
    /// Returns 404 if `course_id` is not a live course, and the store's error on failure.
    pub async fn delete_course_schedule(&self, course_id: &str) -> Result<(), AppError> {
        Ok(self.store().delete_course_schedule(course_id).await?)
    }

    /// Manually redistributes the shared `scheduledDate` of all live units in `course_id`
    /// across `fromDate..=targetDate` proportionally by study amount, announces changed
    /// blocks in the background, and returns the updated schedule view.
    ///
    /// # Errors
    ///
    /// Returns 400 if the redistribution window is missing or invalid, 404 if `course_id`
    /// is not a live course, and the store's error on failure.
    pub async fn redistribute_course_schedule(
        &self,
        course_id: &str,
        request: &RedistributeScheduleRequest,
        user_id: &str,
        enrollment: CourseEnrollment,
    ) -> Result<CourseScheduleView, AppError> {
        let today = self.caller_today(user_id).await?;
        let course = self.store().get_container(course_id).await?;
        let updated_ids = {
            let _structure = self.collab().changing_structure(self, course_id).await;
            self.store()
                .redistribute_course_schedule(course_id, request, today, user_id)
                .await?
        };
        self.metrics()
            .course_schedule_redistributed(ScheduleRedistributionScope::SharedManual);
        if !updated_ids.is_empty() {
            self.announce_blocks(&course.workspace_id, updated_ids);
        }
        self.course_schedule_view_at(course_id, user_id, enrollment, today)
            .await
    }

    /// Manually redistributes only `user_id`'s incomplete live units in `course_id`,
    /// writing only `user_id`'s personal `unit_progress.planned_date` rows with no
    /// broadcast, webhook, search, or audit side effects.
    ///
    /// # Errors
    ///
    /// Returns 400 if the redistribution window is missing or invalid, 404 if `course_id`
    /// is not a live course, and the store's error on failure.
    pub async fn redistribute_my_course_schedule(
        &self,
        course_id: &str,
        request: &RedistributeScheduleRequest,
        user_id: &str,
    ) -> Result<CourseScheduleView, AppError> {
        let today = self.caller_today(user_id).await?;
        self.store()
            .redistribute_personal_schedule(course_id, user_id, request, today)
            .await?;
        self.metrics()
            .course_schedule_redistributed(ScheduleRedistributionScope::Personal);
        self.course_schedule_view_at(course_id, user_id, CourseEnrollment::Enrolled, today)
            .await
    }

    /// Sets or clears (`None`) `user_id`'s personal `planned_date` on a live unit of
    /// `course_id`, preserving `completed_at` and `last_position`.
    ///
    /// # Errors
    ///
    /// Returns 404 if `unit_id` is not a live unit of `course_id`, and the store's error
    /// if the write fails.
    pub async fn put_unit_schedule(
        &self,
        course_id: &str,
        unit_id: &str,
        user_id: &str,
        planned_date: Option<DiaryDate>,
    ) -> Result<UnitProgress, AppError> {
        Ok(self
            .store()
            .set_unit_planned_date(course_id, unit_id, user_id, planned_date)
            .await?)
    }

    /// Returns `user_id`'s incomplete units due today and overdue across all enrolled
    /// live courses in `workspace_id`, evaluated in `user_id`'s configured time zone.
    ///
    /// # Errors
    ///
    /// Returns the store's error if settings or courses cannot be read.
    pub async fn workspace_learning_today(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> Result<WorkspaceLearningToday, AppError> {
        let today = self.caller_today(user_id).await?;
        Ok(self
            .store()
            .get_workspace_learning_today(workspace_id, user_id, today)
            .await?)
    }
}
