//! Course study schedules, shared/personal redistribution, personal unit planned dates,
//! and workspace today/overdue learning feeds.
//!
//! All routes live under `/w/{workspaceId}/...` behind [`InCourse`], enforcing session
//! authentication, workspace membership, SSO policy, personal-space password
//! confirmation, and `ContainerKind::Course` isolation.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{
    CourseScheduleUpdate, CourseScheduleView, RedistributeScheduleRequest, UnitProgress,
    UnitScheduleUpdate, WorkspaceLearningToday,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{CourseId, MemberWorkspaceId, UnitId};
use super::contract::tag;
use super::unit_progress::course_view_access;
use super::workspace::InCourse;
use super::{
    ApiResult, decode_object, json_response, no_content, require_container_permission,
    require_granted,
};
use super::{Delegated, Read};
use crate::app::{App, ContainerAccess, CourseEnrollment};
use crate::error::AppError;
use crate::permissions::{ContainerPermission, Grant};

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(
            handle_get_course_schedule,
            handle_put_course_schedule,
            handle_delete_course_schedule
        ))
        .routes(routes!(handle_redistribute_course_schedule))
        .routes(routes!(handle_redistribute_my_course_schedule))
        .routes(routes!(handle_put_unit_schedule))
        .routes(routes!(handle_get_workspace_learning_today))
}

fn enrollment_of(access: &ContainerAccess) -> CourseEnrollment {
    if access.has_membership() {
        CourseEnrollment::Enrolled
    } else {
        CourseEnrollment::Unjoined
    }
}

/// Loads course access and requires `ManageCards` permission (course editor/admin or
/// personal-space owner), returning 404 for hidden/non-course containers and 403 for
/// viewers/commenters/unjoined readers.
async fn course_edit_access(
    app: &App,
    user_id: &str,
    course_id: &str,
    denial: &str,
) -> Result<CourseEnrollment, AppError> {
    let access = app.container_access(user_id, course_id).await?;
    let grant = access.as_ref().map_or(Grant::NONE, |found| found.grant);
    require_granted(grant, ContainerPermission::ManageCards, denial)?;
    let access = access.ok_or_else(|| AppError::not_found("course"))?;
    Ok(enrollment_of(&access))
}

/// Schedule of a course
///
/// Returns the course's shared schedule settings (`schedule`, or `null` if unset),
/// today's calendar date (`today`) computed in the caller's configured `timeZone` (UTC
/// when set to `auto`), `todayCount`, `overdueCount`, and every live unit in outline
/// tree order (chapters and trashed units excluded). For enrolled members and
/// personal-space owners, each unit includes the caller's own `myPlannedDate`,
/// `effectiveDate` (`myPlannedDate ?? scheduledDate`), `completed`, and `overdue`
/// (`!completed && effectiveDate < today`). Unjoined readers of an open course receive
/// `myPlannedDate: null` and `completed: false`. Other users' personal planned dates are
/// never disclosed. Overdue units never auto-shift when read.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses/{courseId}/schedule",
    operation_id = "getCourseSchedule",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId),
    responses((status = 200, description = "The course schedule and live unit dates for the caller.", body = CourseScheduleView)),
)]
async fn handle_get_course_schedule(
    Delegated(InCourse(ws), ..): Delegated<Read, InCourse>,
    Path((_, course_id)): Path<(String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to course schedule",
    )
    .await?;
    let view = ws
        .app
        .course_schedule_view(&course_id, user_id, enrollment_of(&access))
        .await?;
    Ok(json_response(&view))
}

/// Save a course's shared schedule
///
/// Creates or updates the course's shared study period (`startDate`..=`targetDate`) and
/// weekday bitmask (`studyDays`, 1..=127 where bit 0 is Monday and bit 6 is Sunday). For
/// editors and admins of the course (and personal-space owners); viewers and unjoined
/// readers are refused with 403. When `autoAssign` is `true`, proportionally assigns
/// shared `scheduledDate` values across all live units in outline tree order by study
/// amount (default weight 1) and announces modified blocks to websocket listeners and
/// webhooks in the background.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/courses/{courseId}/schedule",
    operation_id = "putCourseSchedule",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId),
    request_body = CourseScheduleUpdate,
    responses((status = 200, description = "The updated course schedule view.", body = CourseScheduleView)),
)]
async fn handle_put_course_schedule(
    InCourse(ws): InCourse,
    Path((_, course_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let enrollment = course_edit_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to edit course schedule",
    )
    .await?;
    let update: CourseScheduleUpdate = decode_object(&body)?;
    let validated = update.check_request()?;
    let view = ws
        .app
        .put_course_schedule(&course_id, &validated, user_id, enrollment)
        .await?;
    Ok(json_response(&view))
}

/// Remove a course's shared schedule
///
/// Removes the course's saved shared schedule settings (idempotent if none exists), for
/// editors and admins of the course (and personal-space owners). Existing unit
/// `scheduledDate` values and learners' personal `plannedDate` rows are left intact.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/courses/{courseId}/schedule",
    operation_id = "deleteCourseSchedule",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId),
    responses((status = 204, description = "The shared course schedule is removed.")),
)]
async fn handle_delete_course_schedule(
    InCourse(ws): InCourse,
    Path((_, course_id)): Path<(String, String)>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &course_id,
        ContainerPermission::ManageCards,
        "access denied to delete course schedule",
    )
    .await?;
    ws.app.delete_course_schedule(&course_id).await?;
    Ok(no_content())
}

/// Redistribute a course's shared schedule
///
/// Explicitly redistributes the shared `scheduledDate` of all live units in outline tree
/// order from `fromDate` (default: caller's today in their configured `timeZone`) to
/// `targetDate` (default: course's saved `targetDate`) across `studyDays` (default:
/// course's saved `studyDays`), weighted by each unit's study `amount` (default 1). For
/// editors and admins of the course (and personal-space owners); viewers and unjoined
/// readers are refused with 403. Announces modified blocks to websocket listeners and
/// webhooks in the background and never reads any user's personal progress.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/schedule/redistribute",
    operation_id = "redistributeCourseSchedule",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId),
    request_body = RedistributeScheduleRequest,
    responses((status = 200, description = "The course schedule view after shared redistribution.", body = CourseScheduleView)),
)]
async fn handle_redistribute_course_schedule(
    InCourse(ws): InCourse,
    Path((_, course_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let enrollment = course_edit_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to redistribute course schedule",
    )
    .await?;
    let request: RedistributeScheduleRequest = decode_object(&body)?;
    let view = ws
        .app
        .redistribute_course_schedule(&course_id, &request, user_id, enrollment)
        .await?;
    Ok(json_response(&view))
}

/// Redistribute my personal schedule in a course
///
/// Explicitly redistributes only the caller's incomplete live units in outline tree order
/// from `fromDate` (default: caller's today in their configured `timeZone`) to
/// `targetDate` (default: course's saved `targetDate`) across `studyDays` (default:
/// course's saved `studyDays`), updating only the caller's own `unit_progress.plannedDate`
/// rows. Completed units and shared unit `scheduledDate` values are untouched. Available
/// to any enrolled course member (including viewers and invited guests) and
/// personal-space owners; unjoined readers of an open course are refused with 403.
/// Personal redistributions never broadcast over websockets, never enter search indexes
/// or archives, and emit no webhooks or audit logs.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/my-schedule/redistribute",
    operation_id = "redistributeMyCourseSchedule",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId),
    request_body = RedistributeScheduleRequest,
    responses((status = 200, description = "The course schedule view after personal redistribution.", body = CourseScheduleView)),
)]
async fn handle_redistribute_my_course_schedule(
    InCourse(ws): InCourse,
    Path((_, course_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to redistribute personal schedule",
    )
    .await?;
    if !access.has_membership() {
        return Err(AppError::forbidden(
            "join the course before redistributing your personal schedule",
        ));
    }
    let request: RedistributeScheduleRequest = decode_object(&body)?;
    let view = ws
        .app
        .redistribute_my_course_schedule(&course_id, &request, user_id)
        .await?;
    Ok(json_response(&view))
}

/// Set or clear my personal planned date on a unit
///
/// Sets the caller's personal `plannedDate` (`YYYY-MM-DD`) on a live unit of the course,
/// or clears it (`null`) to revert to the unit's shared `scheduledDate`. Preserves
/// `completedAt` and `lastPosition`. Available to any enrolled course member (including
/// viewers and invited guests) and personal-space owners; unjoined readers of an open
/// course are refused with 403. Never visible to other members, workspace/org admins, or
/// public link readers, and emits no websocket broadcasts, webhooks, search updates, or
/// audit logs. Chapters, trashed units, units of another course, and non-course
/// containers answer 404.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/schedule",
    operation_id = "putUnitSchedule",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId),
    request_body = UnitScheduleUpdate,
    responses((status = 200, description = "The updated personal progress row with the new plannedDate.", body = UnitProgress)),
)]
async fn handle_put_unit_schedule(
    InCourse(ws): InCourse,
    Path((_, course_id, unit_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let access = course_view_access(
        &ws.app,
        user_id,
        &course_id,
        "access denied to update unit schedule",
    )
    .await?;
    if !access.has_membership() {
        return Err(AppError::forbidden(
            "join the course before setting a personal planned date",
        ));
    }
    let update: UnitScheduleUpdate = decode_object(&body)?;
    let planned_date = update.check_request()?;
    let progress = ws
        .app
        .put_unit_schedule(&course_id, &unit_id, user_id, planned_date)
        .await?;
    Ok(json_response(&progress))
}

/// Today's and overdue learning units in a workspace
///
/// Returns the caller's incomplete units due today (`effectiveDate == today`) and
/// overdue (`effectiveDate < today`) across all live courses in this workspace that the
/// caller is enrolled in (direct membership or inherited group role), evaluated in the
/// caller's configured `timeZone` (UTC when set to `auto`). `todayUnits` are ordered by
/// course creation order (`createdAt`, `courseId`) and then outline order within each
/// course; `overdueUnits` are ordered by `effectiveDate` ascending (oldest first), then
/// course creation order, then outline order. Never includes private courses the caller
/// is not enrolled in (even for workspace or organization admins) and never reads
/// another user's personal `plannedDate` or completion status.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/learning/today",
    operation_id = "getWorkspaceLearningToday",
    tag = tag::COURSES,
    params(MemberWorkspaceId),
    responses((status = 200, description = "The caller's incomplete units due today and overdue in this workspace.", body = WorkspaceLearningToday)),
)]
async fn handle_get_workspace_learning_today(
    Delegated(InCourse(ws), ..): Delegated<Read, InCourse>,
) -> ApiResult {
    let feed = ws
        .app
        .workspace_learning_today(&ws.workspace_id, &ws.caller.user_id)
        .await?;
    Ok(json_response(&feed))
}
