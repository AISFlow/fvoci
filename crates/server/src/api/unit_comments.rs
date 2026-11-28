//! The comments on a unit of a course: listing, writing, changing, deleting and
//! restoring them, and what one said before.

use axum::body::Bytes;
use fvoci_model::{Block, Body, BodyChange, CommentVersion, CommentWithReactions};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::comments::{
    CommenterOf, comment_versions_of, create_comment_of, delete_comment_of, list_comments_of,
    patch_comment_of, restore_comment_of,
};
use super::contract::params::{CourseId, MemberWorkspaceId, UnitCommentId, UnitId};
use super::contract::tag;
use super::items::{ItemOf, UnitItem};

use super::{ApiResult, Delegated, Read, Write};
use crate::app::{App, ChangeNotification};

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
/// parent is the unit, each with its `reactions` where it has any. For who reads the
/// course: its members, and of an open course whoever works in the workspace. A
/// chapter or a unit in the trash is not there (404).
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/comments",
    operation_id = "getUnitComments",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId),
    responses((
        status = 200,
        description = "The comments, oldest first.",
        body = Vec<CommentWithReactions>,
    )),
)]
async fn handle_get_unit_comments(
    Delegated(unit, ..): Delegated<Read, ItemOf<UnitItem>>,
) -> ApiResult {
    list_comments_of(unit).await
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
    Delegated(commenter, ..): Delegated<Write, CommenterOf<UnitItem>>,
    body: Bytes,
) -> ApiResult {
    create_comment_of::<_, NewUnitComment>(commenter, &body, ChangeNotification::Send).await
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
    Delegated(commenter, ..): Delegated<Write, CommenterOf<UnitItem>>,
    body: Bytes,
) -> ApiResult {
    patch_comment_of::<_, UnitCommentPatch>(commenter, &body, ChangeNotification::Send, |patch| {
        patch.read_at
    })
    .await
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
    Delegated(commenter, ..): Delegated<Write, CommenterOf<UnitItem>>,
) -> ApiResult {
    delete_comment_of(commenter, ChangeNotification::Send).await
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
async fn handle_restore_unit_comment(commenter: CommenterOf<UnitItem>) -> ApiResult {
    restore_comment_of(commenter).await
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
    Delegated(unit, ..): Delegated<Read, ItemOf<UnitItem>>,
) -> ApiResult {
    comment_versions_of(unit).await
}
