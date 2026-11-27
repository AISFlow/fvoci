//! Course study meetings, shared attendance, exams, and strictly personal exam scores.
//!
//! All routes live under `/w/{workspaceId}/courses/{courseId}/...` behind [`InCourse`],
//! enforcing session authentication, workspace membership, SSO policy, personal-space
//! password confirmation, and `ContainerKind::Course` isolation.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{
    CourseExam, CourseExamPatch, CourseMeeting, CourseMeetingPatch, MeetingAttendance,
    MeetingAttendanceUpdate, MyExamScore, MyExamScoreUpdate, NewCourseExam, NewCourseMeeting,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{CourseId, ExamId, MeetingId, MemberWorkspaceId, UserId};
use super::contract::tag;
use super::workspace::InCourse;
use super::{
    ApiResult, created, decode_object, json_response, no_content, require_container_permission,
    require_granted,
};
use super::{Delegated, Read};
use crate::app::{App, ContainerAccess};
use crate::error::AppError;
use crate::permissions::{ContainerPermission, Grant};

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(
            handle_list_course_meetings,
            handle_create_course_meeting
        ))
        .routes(routes!(
            handle_patch_course_meeting,
            handle_delete_course_meeting
        ))
        .routes(routes!(handle_put_meeting_attendance))
        .routes(routes!(handle_list_course_exams, handle_create_course_exam))
        .routes(routes!(handle_patch_course_exam, handle_delete_course_exam))
        .routes(routes!(
            handle_put_my_exam_score,
            handle_delete_my_exam_score
        ))
        .routes(routes!(
            handle_publish_course_exam_results,
            handle_unpublish_course_exam_results
        ))
}

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

/// Study meetings of a course
///
/// Returns every study meeting of the course ordered by `startsAt` ascending, then by `id`.
/// Enrolled course members and course role managers (`ManageRoles`) receive per-member
/// `attendance` records (ordered by `userId`), with `updatedAt` populated only for
/// `ManageRoles` managers (`0` for non-manager members); unjoined open-course readers and
/// non-enrolled guests receive `attendance: []`. Non-course containers and
/// unreadable private courses answer 404.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses/{courseId}/meetings",
    operation_id = "listCourseMeetings",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId),
    responses((status = 200, description = "The study meetings of the course.", body = Vec<CourseMeeting>)),
)]
async fn handle_list_course_meetings(
    Delegated(InCourse(ws), ..): Delegated<Read, InCourse>,
    Path((_, course_id)): Path<(String, String)>,
) -> ApiResult {
    let access = course_view_access(
        &ws.app,
        &ws.caller.user_id,
        &course_id,
        "access denied to course meetings",
    )
    .await?;
    let mut meetings = ws.app.list_course_meetings(&course_id).await?;
    let is_manager = access.grant.allows(ContainerPermission::ManageRoles);
    if !access.has_membership() && !is_manager {
        for meeting in &mut meetings {
            meeting.attendance.clear();
        }
    } else if !is_manager {
        for meeting in &mut meetings {
            for record in &mut meeting.attendance {
                record.updated_at = 0;
            }
        }
    }
    Ok(json_response(&meetings))
}

/// Create a study meeting in a course
///
/// Creates a study meeting on the course. Requires editor permission or above on the
/// course (or ownership of a personal-space course); viewers, commenters, and unjoined
/// open-course readers are refused with 403. Non-course containers, unreadable private
/// courses, and `presenterId`/`hostId` values that are not active enrolled course members
/// answer 404.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/meetings",
    operation_id = "createCourseMeeting",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId),
    request_body = NewCourseMeeting,
    responses((status = 201, description = "The created study meeting.", body = CourseMeeting)),
)]
async fn handle_create_course_meeting(
    InCourse(ws): InCourse,
    Path((_, course_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &course_id,
        ContainerPermission::ManageCards,
        "access denied to create course meeting",
    )
    .await?;
    let req: NewCourseMeeting = decode_object(&body)?;
    let validated = req.check_request()?;
    let meeting = ws.app.create_course_meeting(&course_id, &validated).await?;
    Ok(created(&meeting))
}

/// Update a study meeting in a course
///
/// Updates fields of a study meeting. Omitted fields are left unchanged; sending `null`
/// for `presenterId` or `hostId` clears that assignment. Requires editor permission or
/// above on the course (or ownership of a personal-space course).
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/courses/{courseId}/meetings/{meetingId}",
    operation_id = "patchCourseMeeting",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, MeetingId),
    request_body = CourseMeetingPatch,
    responses((status = 200, description = "The updated study meeting.", body = CourseMeeting)),
)]
async fn handle_patch_course_meeting(
    InCourse(ws): InCourse,
    Path((_, course_id, meeting_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &course_id,
        ContainerPermission::ManageCards,
        "access denied to update course meeting",
    )
    .await?;
    let patch: CourseMeetingPatch = decode_object(&body)?;
    let change = patch.check_request()?;
    let meeting = ws
        .app
        .patch_course_meeting(&course_id, &meeting_id, &change)
        .await?;
    Ok(json_response(&meeting))
}

/// Delete a study meeting in a course
///
/// Permanently deletes a study meeting and its attendance records. Requires editor
/// permission or above on the course (or ownership of a personal-space course).
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/courses/{courseId}/meetings/{meetingId}",
    operation_id = "deleteCourseMeeting",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, MeetingId),
    responses((status = 204, description = "The study meeting was deleted.")),
)]
async fn handle_delete_course_meeting(
    InCourse(ws): InCourse,
    Path((_, course_id, meeting_id)): Path<(String, String, String)>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &course_id,
        ContainerPermission::ManageCards,
        "access denied to delete course meeting",
    )
    .await?;
    ws.app
        .delete_course_meeting(&course_id, &meeting_id)
        .await?;
    Ok(no_content())
}

/// Record attendance on a course study meeting
///
/// Creates or updates `userId`'s attendance status (`present`, `late`, `absent`, or
/// `excused`) on the meeting.
///
/// Permission rules:
/// - Course editors, course admins, and personal-space owners may record attendance for
///   any active enrolled course member.
/// - Any enrolled course member (including viewers, commenters, and invited guests) may
///   record their own attendance when `userId` matches the caller.
/// - Unjoined open-course readers, and viewers/commenters attempting to modify another
///   member's attendance, are refused with 403.
/// - Non-course containers, unreadable private courses, unknown meetings, and target
///   users who are not active enrolled course members answer 404.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/courses/{courseId}/meetings/{meetingId}/attendance/{userId}",
    operation_id = "putMeetingAttendance",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, MeetingId, UserId),
    request_body = MeetingAttendanceUpdate,
    responses((status = 200, description = "The updated attendance record.", body = MeetingAttendance)),
)]
async fn handle_put_meeting_attendance(
    InCourse(ws): InCourse,
    Path((_, course_id, meeting_id, target_user_id)): Path<(String, String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let caller_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        caller_id,
        &course_id,
        "access denied to record meeting attendance",
    )
    .await?;
    let can_edit_any = access.grant.allows(ContainerPermission::ManageCards);
    let can_edit_self = caller_id == target_user_id && access.has_membership();
    if !can_edit_any && !can_edit_self {
        return Err(AppError::forbidden(
            "only course editors or the member themselves may update attendance",
        ));
    }
    let update: MeetingAttendanceUpdate = decode_object(&body)?;
    let attendance = ws
        .app
        .put_meeting_attendance(&course_id, &meeting_id, &target_user_id, update.status)
        .await?;
    Ok(json_response(&attendance))
}

/// Exams of a course
///
/// Returns every exam of the course ordered by `examDate` ascending (exams without a date
/// last), then by `createdAt` and `id`. Enrolled course managers (`ManageRoles`) receive
/// the exact `scoreCount` across active enrolled course members; other enrolled members
/// receive `scoreCount` rounded down to a multiple of `5` so single-student score
/// submissions are not exposed. Every enrolled member receives `myScore` containing only
/// their own score and — once a course manager publishes a result snapshot
/// (`publishCourseExamResults`) — the frozen coarse snapshot (`averageScore` rounded to
/// one decimal place, `resultsPublishedAt`, and `publishedScoreBucket` rounded down to a
/// multiple of `5`). Before publication, `averageScore` is `null` for every role
/// (including admins). Unjoined readers receive `myScore: null`, `averageScore: null`,
/// and `scoreCount: 0`. Another member's individual score or note is never returned under
/// any role.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses/{courseId}/exams",
    operation_id = "listCourseExams",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId),
    responses((status = 200, description = "The exams of the course.", body = Vec<CourseExam>)),
)]
async fn handle_list_course_exams(
    Delegated(InCourse(ws), ..): Delegated<Read, InCourse>,
    Path((_, course_id)): Path<(String, String)>,
) -> ApiResult {
    let caller_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        caller_id,
        &course_id,
        "access denied to course exams",
    )
    .await?;
    let exams = ws
        .app
        .list_course_exams(&course_id, &access, caller_id)
        .await?;
    Ok(json_response(&exams))
}

/// Create an exam in a course
///
/// Creates an exam definition on the course. Requires editor permission or above on the
/// course (or ownership of a personal-space course).
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/exams",
    operation_id = "createCourseExam",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId),
    request_body = NewCourseExam,
    responses((status = 201, description = "The created exam.", body = CourseExam)),
)]
async fn handle_create_course_exam(
    InCourse(ws): InCourse,
    Path((_, course_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &course_id,
        ContainerPermission::ManageCards,
        "access denied to create course exam",
    )
    .await?;
    let req: NewCourseExam = decode_object(&body)?;
    let validated = req.check_request()?;
    let exam = ws.app.create_course_exam(&course_id, &validated).await?;
    Ok(created(&exam))
}

/// Update an exam in a course
///
/// Updates fields of a course exam. Omitted fields are left unchanged; sending `null` for
/// `examDate` clears the date. Requires editor permission or above on the course (or
/// ownership of a personal-space course).
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/courses/{courseId}/exams/{examId}",
    operation_id = "patchCourseExam",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, ExamId),
    request_body = CourseExamPatch,
    responses((status = 200, description = "The updated exam.", body = CourseExam)),
)]
async fn handle_patch_course_exam(
    InCourse(ws): InCourse,
    Path((_, course_id, exam_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let caller_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        caller_id,
        &course_id,
        "access denied to update course exam",
    )
    .await?;
    require_granted(
        access.grant,
        ContainerPermission::ManageCards,
        "access denied to update course exam",
    )?;
    let patch: CourseExamPatch = decode_object(&body)?;
    let change = patch.check_request()?;
    let exam = ws
        .app
        .patch_course_exam(&course_id, &exam_id, &access, caller_id, &change)
        .await?;
    Ok(json_response(&exam))
}

/// Delete an exam in a course
///
/// Permanently deletes an exam and all personal score rows recorded for it. Requires
/// editor permission or above on the course (or ownership of a personal-space course).
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/courses/{courseId}/exams/{examId}",
    operation_id = "deleteCourseExam",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, ExamId),
    responses((status = 204, description = "The exam was deleted.")),
)]
async fn handle_delete_course_exam(
    InCourse(ws): InCourse,
    Path((_, course_id, exam_id)): Path<(String, String, String)>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &course_id,
        ContainerPermission::ManageCards,
        "access denied to delete course exam",
    )
    .await?;
    ws.app.delete_course_exam(&course_id, &exam_id).await?;
    Ok(no_content())
}

/// Record my score on a course exam
///
/// Creates or updates the caller's own personal score (`0 <= score <= exam.maxScore`) and
/// optional personal note on the exam. Always writes strictly to the authenticated
/// caller's row; no other user's score can be targeted or read.
///
/// Enrolled course members of any role (including viewers, commenters, and invited
/// guests) and personal-space owners may record their own score; an unjoined reader of an
/// open course is refused with 403. Personal score writes never broadcast over
/// websockets, never enter search indexes or archives, and emit no webhooks,
/// notifications, or audit logs.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/courses/{courseId}/exams/{examId}/my-score",
    operation_id = "putMyExamScore",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, ExamId),
    request_body = MyExamScoreUpdate,
    responses((status = 200, description = "The caller's saved exam score.", body = MyExamScore)),
)]
async fn handle_put_my_exam_score(
    InCourse(ws): InCourse,
    Path((_, course_id, exam_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to record exam score",
    )
    .await?;
    if !access.has_membership() {
        return Err(AppError::forbidden(
            "join the course before recording an exam score",
        ));
    }
    let update: MyExamScoreUpdate = decode_object(&body)?;
    let validated = update.check_request()?;
    let score = ws
        .app
        .put_my_exam_score(&course_id, &exam_id, user_id, &validated)
        .await?;
    Ok(json_response(&score))
}

/// Delete my score on a course exam
///
/// Deletes the caller's own personal score on the exam (idempotent if none exists).
/// Enrolled course members and personal-space owners may delete their own score; unjoined
/// open-course readers are refused with 403.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/courses/{courseId}/exams/{examId}/my-score",
    operation_id = "deleteMyExamScore",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, ExamId),
    responses((status = 204, description = "The caller's exam score was deleted.")),
)]
async fn handle_delete_my_exam_score(
    InCourse(ws): InCourse,
    Path((_, course_id, exam_id)): Path<(String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to delete exam score",
    )
    .await?;
    if !access.has_membership() {
        return Err(AppError::forbidden(
            "join the course before deleting an exam score",
        ));
    }
    ws.app
        .delete_my_exam_score(&course_id, &exam_id, user_id)
        .await?;
    Ok(no_content())
}

/// Publish a course exam's aggregate result snapshot
///
/// Freezes and publishes a coarse result snapshot (`averageScore` rounded to `0.1` and
/// `publishedScoreBucket` rounded down to a multiple of `5`) on the exam. Requires course
/// role management (`ManageRoles`) and at least 5 active enrolled scorers. When a previous
/// snapshot exists, recomputing requires at least 3 more scorers than it counted, so a
/// smaller or shrunken count never yields a new snapshot (`409` if already published;
/// restores the frozen snapshot without recomputing if currently unpublished).
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/exams/{examId}/results-publication",
    operation_id = "publishCourseExamResults",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, ExamId),
    responses((status = 200, description = "The exam with its published result snapshot.", body = CourseExam)),
)]
async fn handle_publish_course_exam_results(
    InCourse(ws): InCourse,
    Path((_, course_id, exam_id)): Path<(String, String, String)>,
) -> ApiResult {
    let caller_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        caller_id,
        &course_id,
        "access denied to publish course exam results",
    )
    .await?;
    require_granted(
        access.grant,
        ContainerPermission::ManageRoles,
        "access denied to publish course exam results",
    )?;
    let exam = ws
        .app
        .publish_course_exam_results(&course_id, &exam_id, &access, caller_id)
        .await?;
    Ok(json_response(&exam))
}

/// Unpublish a course exam's aggregate result snapshot
///
/// Hides the published result snapshot on the exam so `averageScore`,
/// `resultsPublishedAt`, and `publishedScoreBucket` return to `null` while retaining the
/// snapshot baseline against republication differencing. Requires course role management
/// (`ManageRoles`).
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/courses/{courseId}/exams/{examId}/results-publication",
    operation_id = "unpublishCourseExamResults",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, ExamId),
    responses((status = 200, description = "The exam with its result snapshot unpublished.", body = CourseExam)),
)]
async fn handle_unpublish_course_exam_results(
    InCourse(ws): InCourse,
    Path((_, course_id, exam_id)): Path<(String, String, String)>,
) -> ApiResult {
    let caller_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        caller_id,
        &course_id,
        "access denied to unpublish course exam results",
    )
    .await?;
    require_granted(
        access.grant,
        ContainerPermission::ManageRoles,
        "access denied to unpublish course exam results",
    )?;
    let exam = ws
        .app
        .unpublish_course_exam_results(&course_id, &exam_id, &access, caller_id)
        .await?;
    Ok(json_response(&exam))
}
