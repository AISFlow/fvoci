//! The comments on a unit of a course: listing, writing, changing, deleting and
//! restoring them, and what one said before.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{Block, Body, BodyChange, CommentVersion, ContainerKinds};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::comments::{
    CommentParentKind, CommenterOf, comments_on, create_comment_on, created_comment,
    delete_comment_on, existing_comment_on, find_live_comment_on, get_comment_versions_on,
    patch_comment_on, restore_comment_on,
};
use super::contract::params::{CourseId, MemberWorkspaceId, UnitCommentId, UnitId};
use super::contract::tag;
use super::courses::{READ_DENIED, content_unit_with_permission};
use super::workspace::{InCourse, InWorkspace};
use super::{ApiResult, json_response};
use super::{Delegated, Read, Write};
use crate::app::{App, ChangeNotification};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(
            handle_get_unit_comments,
            handle_create_unit_comment
        ))
        .routes(routes!(
            handle_patch_unit_comment,
            handle_delete_unit_comment
        ))
        .routes(routes!(handle_get_unit_comment_versions))
        .routes(routes!(handle_restore_unit_comment))
}

const CREATE_DENIED: &str = "access denied to comment on the unit";
const CHANGES_DENIED: &str = "access denied to change the comments of the unit";

pub(super) struct UnitCommentParent;

impl CommentParentKind for UnitCommentParent {
    const CONTAINER_PARAM: &'static str = "courseId";
    const PARENT_PARAM: &'static str = "unitId";
    const CREATE_DENIED: &'static str = CREATE_DENIED;
    const CHANGES_DENIED: &'static str = CHANGES_DENIED;

    fn kinds() -> ContainerKinds {
        ContainerKinds::COURSES
    }

    async fn load_parent(
        ws: &InWorkspace,
        course_id: &str,
        unit_id: &str,
    ) -> Result<Block, AppError> {
        ws.app.content_unit_of(course_id, unit_id).await
    }
}

/// A comment to write on a unit.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NewUnitComment {
    /// The comment's text, as a unit's body is written; neither empty nor `null`.
    #[serde(default)]
    #[schema(value_type = Body)]
    #[expect(
        dead_code,
        reason = "body is extracted via decode_with_document while ToSchema exposes it"
    )]
    body: BodyChange,
}

/// What a comment on a unit is changed to: its text, written over the comment as it was
/// read.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UnitCommentPatch {
    /// The comment's new text, as a unit's body is written; neither empty nor `null`.
    #[serde(default)]
    #[schema(value_type = Body)]
    #[expect(
        dead_code,
        reason = "body is extracted via decode_with_document while ToSchema exposes it"
    )]
    body: BodyChange,
    /// The `updatedAt` of the comment as the client read it: if the comment has changed
    /// since, the patch is refused with 409 and changes nothing.
    #[schema(value_type = i64)]
    read_at: Option<i64>,
}

/// The comments on a unit
///
/// Every comment on the unit, oldest first, as blocks of the `type` `comment` whose
/// parent is the unit. For who reads the course: its members, and of an open course
/// whoever works in the workspace. A chapter or a unit in the trash is not there (404).
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/comments",
    operation_id = "getUnitComments",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId),
    responses((status = 200, description = "The comments, oldest first.", body = Vec<Block>)),
)]
async fn handle_get_unit_comments(
    Delegated(InCourse(ws), ..): Delegated<Read, InCourse>,
    Path((_, course_id, unit_id)): Path<(String, String, String)>,
) -> ApiResult {
    Ok(json_response(
        &unit_comments(&ws, &course_id, &unit_id).await?,
    ))
}

/// The comments on the unit `unit_id` of the course `course_id`, as `getUnitComments`
/// lists them.
///
/// # Errors
///
/// As `getUnitComments` answers.
pub(super) async fn unit_comments(
    ws: &InWorkspace,
    course_id: &str,
    unit_id: &str,
) -> Result<Vec<Block>, AppError> {
    let unit = content_unit_with_permission(
        ws,
        course_id,
        unit_id,
        ContainerPermission::View,
        READ_DENIED,
    )
    .await?;
    comments_on(ws, course_id, &unit.id).await
}

/// Write a comment on a unit
///
/// For the course's members from the commenter's role up (whoever reads an open course
/// without joining it is refused with 403).
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/comments",
    operation_id = "createUnitComment",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId),
    request_body = NewUnitComment,
    responses((status = 201, description = "The comment as it was stored.", body = Block)),
)]
async fn handle_create_unit_comment(
    Delegated(commenter, ..): Delegated<Write, CommenterOf<UnitCommentParent>>,
    body: Bytes,
) -> ApiResult {
    let comment = create_comment_on::<NewUnitComment>(
        &commenter.ws,
        &commenter.container_id,
        commenter.parent,
        "unit",
        &body,
        ChangeNotification::Send,
    )
    .await?;
    Ok(created_comment(&comment))
}

/// Change a comment on a unit
///
/// Writes a new text over the comment: by its author from the commenter's role up, and
/// the comment of somebody else by the course's admins only (403).
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/comments/{commentId}",
    operation_id = "patchUnitComment",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId, UnitCommentId),
    request_body = UnitCommentPatch,
    responses((status = 200, description = "The comment as it is now.", body = Block)),
)]
async fn handle_patch_unit_comment(
    Delegated(commenter, ..): Delegated<Write, CommenterOf<UnitCommentParent>>,
    body: Bytes,
) -> ApiResult {
    let comment_id = commenter.comment_id()?;
    let comment = existing_comment_on(&commenter.ws, &commenter.parent, comment_id).await?;
    let patched = patch_comment_on::<UnitCommentPatch>(
        &commenter.ws,
        &commenter.container_id,
        &comment,
        CHANGES_DENIED,
        &body,
        ChangeNotification::Send,
        |patch| patch.read_at,
    )
    .await?;
    Ok(json_response(&patched))
}

/// Delete a comment on a unit
///
/// The comment goes to the trash: by its author from the commenter's role up, and the
/// comment of somebody else by the course's admins only (403).
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/comments/{commentId}",
    operation_id = "deleteUnitComment",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId, UnitCommentId),
    responses((status = 204, description = "The comment is deleted.")),
)]
async fn handle_delete_unit_comment(
    Delegated(commenter, ..): Delegated<Write, CommenterOf<UnitCommentParent>>,
) -> ApiResult {
    let comment_id = commenter.comment_id()?;
    let comment = existing_comment_on(&commenter.ws, &commenter.parent, comment_id).await?;
    delete_comment_on(
        &commenter.ws,
        &comment,
        CHANGES_DENIED,
        ChangeNotification::Send,
    )
    .await
}

/// Bring a deleted comment on a unit back
///
/// Restores a comment deleted on its own onto its unit: for its author from the
/// commenter's role up if they deleted it themselves, and for the course's admins
/// whoever wrote or deleted it.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/comments/{commentId}/restore",
    operation_id = "restoreUnitComment",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId, UnitCommentId),
    responses((status = 200, description = "The comment as it is again.", body = Block)),
)]
async fn handle_restore_unit_comment(commenter: CommenterOf<UnitCommentParent>) -> ApiResult {
    let comment_id = commenter.comment_id()?;
    let comment =
        restore_comment_on(&commenter.ws, &commenter.parent, comment_id, CHANGES_DENIED).await?;
    Ok(json_response(&comment))
}

/// What a comment on a unit said before
///
/// The earlier texts of the comment: every version but the one it has now, the first
/// one first, with who wrote it and when.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/comments/{commentId}/versions",
    operation_id = "getUnitCommentVersions",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId, UnitCommentId),
    responses((status = 200, description = "The earlier versions, the first one first.", body = Vec<CommentVersion>)),
)]
async fn handle_get_unit_comment_versions(
    Delegated(InCourse(ws), ..): Delegated<Read, InCourse>,
    Path((_, course_id, unit_id, comment_id)): Path<(String, String, String, String)>,
) -> ApiResult {
    let unit = content_unit_with_permission(
        &ws,
        &course_id,
        &unit_id,
        ContainerPermission::View,
        READ_DENIED,
    )
    .await?;
    let live = find_live_comment_on(&ws, &unit, &comment_id).await?;
    get_comment_versions_on(&ws, &unit, live, &comment_id).await
}
