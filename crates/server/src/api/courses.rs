//! Courses: listing and making them, and their chapters and units.
//!
//! A course is a container of the kind `course`. What is shared with every container is
//! asked under `/containers/{containerId}` (`InContainer`), and who is on it under
//! `InShared`; here is what only a course has, and the handle these routes work with
//! reaches courses and nothing else (`InCourse`).

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{
    Block, BlockError, Container, ContainerKind, ContainerVisibility, DeletedUnit, MinimumRole,
    NewUnit, Unit, UnitKind, UnitMove, UnitNode, UnitPatch,
};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::Page as PageNumber;
use super::contract::params::{CourseId, MemberWorkspaceId, PerPage, UnitId};
use super::contract::tag;
use super::query::{PagingQuery, QueryParams};
use super::workspace::{InCourse, InWorkspace};
use super::{
    ApiResult, created, decode_object, decode_with_document, json_response, no_content,
    require_container_permission,
};
use super::{Delegated, Read, Write};
use crate::app::{App, ChangeNotification};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_courses, handle_create_course))
        .routes(routes!(handle_get_unit_tree, handle_create_unit))
        .routes(routes!(
            handle_get_unit,
            handle_patch_unit,
            handle_delete_unit
        ))
        .routes(routes!(handle_move_unit))
        .routes(routes!(handle_get_deleted_units))
        .routes(routes!(handle_restore_unit))
}

fn default_course_minimum_role() -> MinimumRole {
    MinimumRole::Commenter
}

/// A course to make.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NewCourse {
    /// Whether the course is `open`, for whoever works in the workspace to read and
    /// search without joining, and to join to enroll, or `private`, for whom it is
    /// shared with.
    visibility: ContainerVisibility,
    /// The role a user gets who joins the course. Left out, `commenter`.
    #[serde(default = "default_course_minimum_role")]
    minimum_role: MinimumRole,
    /// The course's title. Left out, empty.
    #[serde(default)]
    title: String,
    /// The course's description. Left out, empty.
    #[serde(default)]
    description: String,
    /// The course's icon, usually one emoji. Left out, none.
    #[serde(default)]
    icon: String,
}

/// The courses of a workspace
///
/// The courses of the workspace that the user is a member of, and the workspace's open
/// courses; a guest of the workspace gets only the former, an administrator every course
/// of the workspace (the units of a private course they are no member of stay 404 to
/// them). Each is a container of the `kind` `course`. Oldest first. 404 unless the user
/// is a member of the workspace.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses",
    operation_id = "getCourses",
    tag = tag::COURSES,
    params(MemberWorkspaceId),
    responses((status = 200, description = "The courses.", body = Vec<Container>)),
)]
async fn handle_get_courses(Delegated(InCourse(ws), ..): Delegated<Read, InCourse>) -> ApiResult {
    let courses = ws
        .app
        .workspace_containers(&ws.caller.user_id, &ws.workspace_id, ws.role)
        .await?;
    Ok(json_response(&courses))
}

/// Make a course
///
/// The user becomes the course's admin, and the course is filed under the user's default
/// category, as a board is. Whoever works in the workspace makes courses in it; a guest
/// is refused (403). A member the request does not know is 400.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses",
    operation_id = "createCourse",
    tag = tag::COURSES,
    params(MemberWorkspaceId),
    request_body = NewCourse,
    responses((status = 201, description = "The course that was made.", body = Container)),
)]
async fn handle_create_course(InCourse(ws): InCourse, body: Bytes) -> ApiResult {
    ws.require_container_maker("access denied to create course")?;
    let NewCourse {
        visibility,
        minimum_role,
        title,
        description,
        icon,
    } = decode_object(&body)?;
    let course = Container {
        workspace_id: ws.workspace_id.clone(),
        visibility,
        kind: ContainerKind::Course,
        minimum_role,
        title,
        description,
        icon,
        ..Container::default()
    };
    let course = ws
        .app
        .create_container_with_admin(course, &ws.caller.user_id)
        .await?;
    Ok(created(&course))
}

/// What a member of the workspace who may not read the course is refused a unit with.
pub(super) const READ_DENIED: &str = "access denied to fetch unit";

/// Returns the unit or chapter of the course for a handler of
/// `/courses/{courseId}/units/{unitId}`, if the user has the permission on the course.
pub(super) async fn unit_with_permission(
    ws: &InWorkspace,
    course_id: &str,
    unit_id: &str,
    permission: ContainerPermission,
    denial: &str,
) -> Result<Block, AppError> {
    require_container_permission(&ws.app, &ws.caller.user_id, course_id, permission, denial)
        .await?;
    ws.app.unit_of(course_id, unit_id).await
}

/// Returns the unit (not a chapter) of the course for a handler that applies only to units
/// with bodies (comments, body versions), if the user has the permission on the course.
pub(super) async fn content_unit_with_permission(
    ws: &InWorkspace,
    course_id: &str,
    unit_id: &str,
    permission: ContainerPermission,
    denial: &str,
) -> Result<Block, AppError> {
    require_container_permission(&ws.app, &ws.caller.user_id, course_id, permission, denial)
        .await?;
    ws.app.content_unit_of(course_id, unit_id).await
}

/// The outline of a course
///
/// Every chapter and unit of the course without its body, for whoever reads the course:
/// each names the chapter it is below (`parentId`, empty for a chapter or a top-level
/// unit) and its `kind` (`unit` or `chapter`). By `order` as strings compare by code
/// unit, then by `id`: the siblings of one parent are in their order when taken out of
/// the list as it is.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses/{courseId}/units",
    operation_id = "getUnitTree",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId),
    responses((status = 200, description = "The chapters and units.", body = Vec<UnitNode>)),
)]
async fn handle_get_unit_tree(
    Delegated(InCourse(ws), ..): Delegated<Read, InCourse>,
    Path((_, course_id)): Path<(String, String)>,
) -> ApiResult {
    Ok(json_response(&unit_tree_read(&ws, &course_id).await?))
}

/// The chapters and units of the course `course_id` without their bodies, as
/// `getUnitTree` lists them.
///
/// # Errors
///
/// As `getUnitTree` answers.
pub(super) async fn unit_tree_read(
    ws: &InWorkspace,
    course_id: &str,
) -> Result<Vec<UnitNode>, AppError> {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        course_id,
        ContainerPermission::View,
        "access denied to fetch units",
    )
    .await?;
    ws.app.unit_tree(course_id).await
}

/// Make a unit or chapter
///
/// The server chooses the identifier, the creator, the times and the `order`. For
/// editors of the course. With `kind` `unit` (also when left out), the unit goes below
/// `parentId`, a chapter of the same course (400 at `/parentId` otherwise), or at the
/// top of the course. With `kind` `chapter`, the chapter is a title-only heading at the
/// top of the course (`parentId` must be empty; 400 at `/parentId` otherwise) with no
/// body and no room. Among the siblings of that parent it goes right after `after` (400
/// at `/after` if that is no sibling of the same parent), first if `after` is empty, and
/// last if it is left out. A unit may also carry optional study-schedule fields
/// (`amount` `1..=100_000`, `amountUnit`, and shared `scheduledDate` `YYYY-MM-DD`;
/// forbidden on chapters with 400). A unit's body is written by `patchUnit`, or together
/// in its room (`joinDoc`).
///
/// A course holds at most 2,000 units and 200 chapters, those in the trash not counted
/// (chapters do not count toward the 2,000-unit limit): one more unit is 400 with the
/// reason `too_many_units` and the `limit`, and one more chapter is 400 with the reason
/// `too_many_chapters` and the `limit`.
///
/// The course's listeners are told with `blockChanged`, whose block of the `type` `unit`
/// or `chapter` carries its `order` among its `fields`. Any sibling numbered anew to
/// leave room is told of too.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/units",
    operation_id = "createUnit",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId),
    request_body = NewUnit,
    responses((status = 201, description = "The unit or chapter that was made.", body = Unit)),
)]
async fn handle_create_unit(
    Delegated(InCourse(ws), ..): Delegated<Write, InCourse>,
    Path((_, course_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    Ok(created(&create_unit_from(&ws, &course_id, &body).await?))
}

/// Makes the unit or chapter `body` describes, as `createUnit` takes it, in the course
/// `course_id`.
///
/// # Errors
///
/// As `createUnit` answers.
pub(super) async fn create_unit_from(
    ws: &InWorkspace,
    course_id: &str,
    body: &Bytes,
) -> Result<Unit, AppError> {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        course_id,
        ContainerPermission::ManageCards,
        "access denied to create unit",
    )
    .await?;
    let new: NewUnit = decode_object(body)?;
    new.check_request()?;
    let unit = ws.app.create_unit(course_id, &new, user_id).await?;
    tracing::debug!(course_id, unit_id = %unit.id, user_id, "CreateUnit");
    Ok(unit)
}

/// A unit or chapter
///
/// The unit with its body, or the chapter (`body` is `null`), for whoever reads its
/// course.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}",
    operation_id = "getUnit",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId),
    responses((status = 200, description = "The unit or chapter.", body = Unit)),
)]
async fn handle_get_unit(
    Delegated(InCourse(ws), ..): Delegated<Read, InCourse>,
    Path((_, course_id, unit_id)): Path<(String, String, String)>,
) -> ApiResult {
    let block = unit_with_permission(
        &ws,
        &course_id,
        &unit_id,
        ContainerPermission::View,
        READ_DENIED,
    )
    .await?;
    Ok(json_response(&Unit::from(block)))
}

/// Change a unit or chapter
///
/// Its title, or (for a unit) its whole body and optional study-schedule fields (`amount`,
/// `amountUnit`, `scheduledDate`, each clearable with `null`), for editors of the course.
/// A chapter has no body or schedule fields: sending `body`, `amount`, `amountUnit`, or
/// `scheduledDate` on a chapter is 400. A new body is sent with
/// `readAt`, the `updatedAt` of the unit as it was read: 409 if the unit has changed
/// since, and nothing is changed. Where the body is being edited together, what the
/// editors typed is written to the unit first, so a writer who read it before is refused
/// and reads it again. Where the node stands in the course does not change here.
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}",
    operation_id = "patchUnit",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId),
    request_body = UnitPatch,
    responses((status = 200, description = "The unit or chapter as it is now.", body = Unit)),
)]
async fn handle_patch_unit(
    Delegated(InCourse(ws), ..): Delegated<Write, InCourse>,
    Path((_, course_id, unit_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    Ok(json_response(
        &patch_unit_from(&ws, &course_id, &unit_id, &body).await?,
    ))
}

/// Changes the unit or chapter `unit_id` of the course `course_id` as `body` says, as
/// `patchUnit` takes it.
///
/// # Errors
///
/// As `patchUnit` answers.
pub(super) async fn patch_unit_from(
    ws: &InWorkspace,
    course_id: &str,
    unit_id: &str,
    body: &Bytes,
) -> Result<Unit, AppError> {
    let user_id = ws.caller.user_id.as_str();
    let unit = unit_with_permission(
        ws,
        course_id,
        unit_id,
        ContainerPermission::ManageCards,
        "access denied to patch unit",
    )
    .await?;
    let (mut patch, document): (UnitPatch, _) = decode_with_document(body)?;
    patch.body = document;
    let kind = UnitKind::of_block(&unit.block_type).unwrap_or_default();
    if kind == UnitKind::Chapter && !patch.body.is_keep() {
        return Err(AppError::from(BlockError::BodyNotAllowed));
    }
    patch.check_request_for(kind)?;
    ws.app
        .check_body_files(course_id, patch.body.new_body())
        .await?;
    let patched = ws
        .app
        .patch_block_as(
            &unit.id,
            &patch.into_block_patch(),
            user_id,
            ChangeNotification::Send,
            ws.caller.agent(),
        )
        .await?;
    tracing::debug!(course_id, unit_id, user_id, "PatchUnit");
    Ok(Unit::from(patched))
}

/// Move a unit or chapter
///
/// Moves a unit to the top of the course (left out or empty `parentId`) or below a
/// chapter of the same course, right after `after` among the siblings there (empty for
/// first, left out or `null` for last). Moves a chapter among the top-level siblings
/// (`parentId` must be empty; 400 at `/parentId` otherwise), keeping its units below it.
/// For editors of the course. 400 at `/parentId` if the parent is no chapter of the
/// course or is the unit itself; 400 at `/after` if that is no other sibling of the
/// parent. Nothing is changed then.
///
/// The course's listeners are told with `blockChanged` of the moved node and of any
/// sibling numbered anew to leave it room; a chapter's units keep their parent and are
/// not told of.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/move",
    operation_id = "moveUnit",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId),
    request_body = UnitMove,
    responses((status = 200, description = "The unit or chapter where it is now.", body = Unit)),
)]
async fn handle_move_unit(
    InCourse(ws): InCourse,
    Path((_, course_id, unit_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let unit = unit_with_permission(
        &ws,
        &course_id,
        &unit_id,
        ContainerPermission::ManageCards,
        "access denied to move unit",
    )
    .await?;
    let to: UnitMove = decode_object(&body)?;
    let moved = ws.app.move_unit(&course_id, &unit.id, &to, user_id).await?;
    tracing::debug!(course_id, unit_id, user_id, "MoveUnit");
    Ok(json_response(&moved))
}

/// Delete a unit or chapter
///
/// Moves the unit to the trash of its course, or moves the chapter together with every
/// unit below it as one deletion, for editors of the course. The course's listeners are
/// told with `blockChanged` of the deleted block alone, with `deletedAt` set.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}",
    operation_id = "deleteUnit",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId),
    responses((status = 204, description = "The unit or chapter is deleted.")),
)]
async fn handle_delete_unit(
    InCourse(ws): InCourse,
    Path((_, course_id, unit_id)): Path<(String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let unit = unit_with_permission(
        &ws,
        &course_id,
        &unit_id,
        ContainerPermission::ManageCards,
        "access denied to delete unit",
    )
    .await?;
    ws.app
        .delete_block(&unit.id, user_id, ChangeNotification::Send)
        .await?;
    tracing::debug!(course_id, unit_id, user_id, "DeleteUnit");
    Ok(no_content())
}

/// The deleted units and chapters of a course
///
/// One page of the course's chapters and units that are in the trash, each with its
/// identifier, `kind`, title, the chapter it was below, when it was deleted, by whom,
/// and (for a chapter) how many units below it went with it; `restoreUnit` brings one
/// back until the trash is emptied of it. A unit that went with its chapter comes back
/// with that chapter, and is not listed separately. For editors of the course. The last
/// deleted first; the pages cut that order.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses/{courseId}/trash/units",
    operation_id = "getDeletedUnits",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, PageNumber, PerPage),
    responses((status = 200, description = "The deleted chapters and units of the course.", body = Vec<DeletedUnit>)),
)]
async fn handle_get_deleted_units(
    InCourse(ws): InCourse,
    Path((_, course_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<PagingQuery>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &course_id,
        ContainerPermission::ManageCards,
        "access denied to the trash of the course",
    )
    .await?;
    let page = query.into_list_page()?;
    Ok(json_response(
        &ws.app.deleted_units(&course_id, page).await?,
    ))
}

/// Bring back a unit or chapter
///
/// Restores the unit from the course's trash, or restores the chapter together with the
/// units deleted along with it, for editors of the course: below the chapter it was
/// below at the place it had (right after the siblings before it if a sibling took that
/// place since), or, if that chapter is gone (in the trash or purged), last at the top
/// of the course. Held to the course's limits: 400 with the reason `too_many_units` or
/// `too_many_chapters` (each with the `limit`) if restoring would exceed them. A block
/// not in this course's trash, or a unit that went with its chapter (it comes back with
/// that chapter), is 404. The course's listeners are told with `blockChanged` of each
/// block that came back (a chapter before its units, and each unit before its comments)
/// and each sibling numbered anew.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/restore",
    operation_id = "restoreUnit",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId),
    responses((status = 200, description = "The unit or chapter as it is again.", body = Unit)),
)]
async fn handle_restore_unit(
    InCourse(ws): InCourse,
    Path((_, course_id, unit_id)): Path<(String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &course_id,
        ContainerPermission::ManageCards,
        "access denied to undelete unit",
    )
    .await?;
    let unit = ws.app.undelete_unit(&course_id, &unit_id, user_id).await?;
    tracing::debug!(course_id, unit_id, user_id, "UndeleteUnit");
    Ok(json_response(&unit))
}
