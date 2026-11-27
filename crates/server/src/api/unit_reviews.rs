//! Personal spaced-repetition reviews (`unit_reviews`) and personal study notes
//! (`unit_notes`) on course units.
//!
//! All routes live under `/w/{workspaceId}/courses/{courseId}/...` or
//! `/w/{workspaceId}/learning/reviews/today` behind [`InCourse`], enforcing session
//! authentication, workspace membership, SSO policy, personal-space password
//! confirmation, and `ContainerKind::Course` isolation. No public-link route exists.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{
    CompleteUnitReview, CompletedUnitReview, CreateUnitNote, PatchUnitNote, ReviewStatusFilter,
    ScheduleUnitReview, TodayReviewItem, UnitNote, UnitNoteKind, UnitReview,
    course_review_intervals,
};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{
    CourseId, MemberWorkspaceId, NoteId, NoteKind, ReviewId, ReviewStatus, ReviewUnitId, UnitId,
};
use super::contract::tag;
use super::query::{QueryParams, non_empty_opt};
use super::workspace::InCourse;
use super::{ApiResult, created, decode_object, json_response, no_content, require_granted};
use crate::app::{App, ContainerAccess};
use crate::error::AppError;
use crate::permissions::{ContainerPermission, Grant};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct CourseReviewsQuery {
    status: String,
    unit_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct UnitNotesQuery {
    kind: String,
}

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_list_my_course_reviews))
        .routes(routes!(handle_schedule_unit_review))
        .routes(routes!(handle_complete_unit_review))
        .routes(routes!(handle_delete_unit_review))
        .routes(routes!(handle_list_my_unit_notes, handle_create_unit_note))
        .routes(routes!(handle_patch_unit_note, handle_delete_unit_note))
        .routes(routes!(handle_get_workspace_today_reviews))
}

/// Loads course access and requires `View` permission, returning 404 when the course is
/// hidden, not a course, or does not exist.
async fn course_view_access(
    app: &App,
    user_id: &str,
    course_id: &str,
    denial: &str,
) -> Result<ContainerAccess, AppError> {
    let access = app.container_access(user_id, course_id).await?;
    let grant = access.as_ref().map_or(Grant::NONE, |found| found.grant);
    require_granted(grant, ContainerPermission::View, denial)?;
    access.ok_or_else(|| AppError::not_found("course"))
}

/// Ensures the caller is an enrolled member of the course (including viewers, guests, and
/// personal-space owners), refusing unjoined open-course readers with 403.
fn require_enrolled_member(access: &ContainerAccess, denial: &str) -> Result<(), AppError> {
    if !access.has_membership() {
        return Err(AppError::forbidden(denial));
    }
    Ok(())
}

fn parse_review_status(raw: &str) -> Result<ReviewStatusFilter, AppError> {
    if raw.is_empty() {
        return Ok(ReviewStatusFilter::All);
    }
    raw.parse()
        .map_err(|error: fvoci_model::ParseReviewStatusError| {
            AppError::bad_request(error.to_string()).with_parameter("status")
        })
}

fn parse_note_kind(raw: &str) -> Result<Option<UnitNoteKind>, AppError> {
    if raw.is_empty() {
        return Ok(None);
    }
    raw.parse()
        .map(Some)
        .map_err(|error: fvoci_model::ParseUnitNoteKindError| {
            AppError::bad_request(error.to_string()).with_parameter("kind")
        })
}

/// My scheduled reviews in a course
///
/// Returns the caller's own spaced-repetition reviews for live units of the course.
/// Ordered by `scheduledDate` ascending, then `createdAt` ascending, then `id` ascending.
/// Optional query filters: `status` (`pending`, `completed`, or `all`) and `unitId`.
/// Unjoined readers of an open course receive an empty array `[]`. Other users' reviews
/// are never returned. Non-course containers and unreadable private courses answer 404.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses/{courseId}/reviews",
    operation_id = "listMyCourseReviews",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, ReviewStatus, ReviewUnitId),
    responses((status = 200, description = "The caller's personal reviews in the course.", body = Vec<UnitReview>)),
)]
async fn handle_list_my_course_reviews(
    InCourse(ws): InCourse,
    Path((_, course_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<CourseReviewsQuery>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to course reviews",
    )
    .await?;
    let status = parse_review_status(&query.status)?;
    let unit_id = non_empty_opt(&query.unit_id);
    if !access.has_membership() {
        return Ok(json_response(&Vec::<UnitReview>::new()));
    }
    let rows = ws
        .app
        .list_my_course_reviews(&course_id, user_id, status, unit_id.as_deref())
        .await?;
    Ok(json_response(&rows))
}

/// Schedule a personal review for a unit
///
/// Creates a personal spaced-repetition review for the caller on a live unit of the
/// course. Provide `intervalDays` (`1..=3650`), `scheduledDate` (`YYYY-MM-DD`), or both.
/// Enrolled course members (including viewers and invited guests) and personal-space
/// owners may schedule reviews; an unjoined reader of an open course is refused with 403.
/// Chapters, trashed units, units of another course, and non-course containers answer 404.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/reviews",
    operation_id = "scheduleUnitReview",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId),
    request_body = ScheduleUnitReview,
    responses((status = 201, description = "The scheduled personal review.", body = UnitReview)),
)]
async fn handle_schedule_unit_review(
    InCourse(ws): InCourse,
    Path((_, course_id, unit_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to schedule unit review",
    )
    .await?;
    require_enrolled_member(&access, "join the course before scheduling reviews")?;
    let request: ScheduleUnitReview = decode_object(&body)?;
    let now = ws.app.schedule_now_millis();
    let today = ws.app.user_today(user_id, now).await?;
    let new = request.check_request(&today)?;
    let review = ws
        .app
        .schedule_unit_review(&course_id, &unit_id, user_id, &new, Some(now))
        .await?;
    Ok(created(&review))
}

/// Complete a personal review
///
/// Marks the caller's review as completed (idempotently preserving an existing
/// `completedAt`) and returns `suggestedNextIntervalDays` calculated from the review's
/// interval and the course's interval sequence (default `[1, 3, 7, 14, 30]`). Optionally
/// pass `scheduleNextIntervalDays` (`1..=3650`) to schedule the follow-up review in the
/// same transaction. Another user's review identifier, a review on a trashed unit, or a
/// non-course container answers 404.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/reviews/{reviewId}/complete",
    operation_id = "completeUnitReview",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, ReviewId),
    request_body = CompleteUnitReview,
    responses((status = 200, description = "The completed review and suggested next interval.", body = CompletedUnitReview)),
)]
async fn handle_complete_unit_review(
    InCourse(ws): InCourse,
    Path((_, course_id, review_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to complete unit review",
    )
    .await?;
    require_enrolled_member(&access, "join the course before completing reviews")?;
    let request: CompleteUnitReview = if body.is_empty() {
        CompleteUnitReview::default()
    } else {
        decode_object(&body)?
    };
    let now = ws.app.schedule_now_millis();
    let next = if request.schedule_next_interval_days.is_some() {
        let today = ws.app.user_today(user_id, now).await?;
        request.check_request(&today)?
    } else {
        None
    };
    let intervals = course_review_intervals(&access.container.properties);
    let completed = ws
        .app
        .complete_unit_review(
            &course_id,
            &review_id,
            user_id,
            &intervals,
            next.as_ref(),
            Some(now),
        )
        .await?;
    Ok(json_response(&completed))
}

/// Cancel or delete a personal review
///
/// Deletes one of the caller's personal reviews in the course. Another user's review
/// identifier or a missing review answers 404.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/courses/{courseId}/reviews/{reviewId}",
    operation_id = "deleteUnitReview",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, ReviewId),
    responses((status = 204, description = "The personal review is deleted.")),
)]
async fn handle_delete_unit_review(
    InCourse(ws): InCourse,
    Path((_, course_id, review_id)): Path<(String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to delete unit review",
    )
    .await?;
    require_enrolled_member(&access, "join the course before deleting reviews")?;
    ws.app
        .delete_unit_review(&course_id, &review_id, user_id)
        .await?;
    Ok(no_content())
}

/// My personal notes on a unit
///
/// Returns the caller's own question, mistake, and review notes on a live unit of the
/// course. Ordered by `createdAt` descending, then `id` descending. Optional query filter
/// `kind` (`question`, `mistake`, or `review`). Unjoined readers of an open course
/// receive an empty array `[]` for live units. Chapters, trashed units, and non-course
/// containers answer 404.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/notes",
    operation_id = "listMyUnitNotes",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId, NoteKind),
    responses((status = 200, description = "The caller's personal notes on the unit.", body = Vec<UnitNote>)),
)]
async fn handle_list_my_unit_notes(
    InCourse(ws): InCourse,
    Path((_, course_id, unit_id)): Path<(String, String, String)>,
    QueryParams(query): QueryParams<UnitNotesQuery>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let access =
        course_view_access(&ws.app, user_id, &course_id, "access denied to unit notes").await?;
    let kind = parse_note_kind(&query.kind)?;
    if !access.has_membership() {
        let _ = ws.app.content_unit_of(&course_id, &unit_id).await?;
        return Ok(json_response(&Vec::<UnitNote>::new()));
    }
    let notes = ws
        .app
        .list_my_unit_notes(&course_id, &unit_id, user_id, kind)
        .await?;
    Ok(json_response(&notes))
}

/// Create a personal note on a unit
///
/// Creates a personal question, mistake, or review note for the caller on a live unit of
/// the course. Content is trimmed and must contain between 1 and 10,000 characters.
/// Enrolled course members (including viewers and invited guests) and personal-space
/// owners may record notes; unjoined readers of an open course are refused with 403.
/// Chapters, trashed units, and non-course containers answer 404.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/notes",
    operation_id = "createUnitNote",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId),
    request_body = CreateUnitNote,
    responses((status = 201, description = "The created personal note.", body = UnitNote)),
)]
async fn handle_create_unit_note(
    InCourse(ws): InCourse,
    Path((_, course_id, unit_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to create unit note",
    )
    .await?;
    require_enrolled_member(&access, "join the course before recording notes")?;
    let request: CreateUnitNote = decode_object(&body)?;
    let new = request.check_request()?;
    let note = ws
        .app
        .create_unit_note(&course_id, &unit_id, user_id, &new)
        .await?;
    Ok(created(&note))
}

/// Update a personal note
///
/// Updates the `kind` and/or `content` of one of the caller's personal notes in the
/// course. Another user's note identifier, a note on a trashed unit, or a non-course
/// container answers 404.
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/courses/{courseId}/notes/{noteId}",
    operation_id = "patchUnitNote",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, NoteId),
    request_body = PatchUnitNote,
    responses((status = 200, description = "The updated personal note.", body = UnitNote)),
)]
async fn handle_patch_unit_note(
    InCourse(ws): InCourse,
    Path((_, course_id, note_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to update unit note",
    )
    .await?;
    require_enrolled_member(&access, "join the course before updating notes")?;
    let request: PatchUnitNote = decode_object(&body)?;
    let change = request.check_request()?;
    let note = ws
        .app
        .patch_unit_note(&course_id, &note_id, user_id, &change)
        .await?;
    Ok(json_response(&note))
}

/// Delete a personal note
///
/// Deletes one of the caller's personal notes in the course. Another user's note
/// identifier or a missing note answers 404.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/courses/{courseId}/notes/{noteId}",
    operation_id = "deleteUnitNote",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, NoteId),
    responses((status = 204, description = "The personal note is deleted.")),
)]
async fn handle_delete_unit_note(
    InCourse(ws): InCourse,
    Path((_, course_id, note_id)): Path<(String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to delete unit note",
    )
    .await?;
    require_enrolled_member(&access, "join the course before deleting notes")?;
    ws.app
        .delete_unit_note(&course_id, &note_id, user_id)
        .await?;
    Ok(no_content())
}

/// My reviews due today in a workspace
///
/// Returns the caller's own pending reviews whose `scheduledDate` is on or before today
/// (in the caller's configured time zone, or UTC) across all live courses of the
/// workspace in which the caller is an active member. Ordered by `scheduledDate`
/// ascending, then `createdAt` ascending, then `id` ascending. Other users' reviews are
/// never returned.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/learning/reviews/today",
    operation_id = "getWorkspaceTodayReviews",
    tag = tag::COURSES,
    params(MemberWorkspaceId),
    responses((status = 200, description = "The caller's pending reviews due today or overdue.", body = Vec<TodayReviewItem>)),
)]
async fn handle_get_workspace_today_reviews(InCourse(ws): InCourse) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let now = ws.app.schedule_now_millis();
    let today = ws.app.user_today(user_id, now).await?;
    let items = ws
        .app
        .workspace_today_reviews(&ws.workspace_id, user_id, &today)
        .await?;
    Ok(json_response(&items))
}
