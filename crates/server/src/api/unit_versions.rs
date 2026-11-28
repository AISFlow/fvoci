//! The versions kept of a unit's body: listing them, reading one, keeping one now, and
//! going back to one.
//!
//! A unit's body is kept as a card's and a page's are (`crates/server/src/collab`), and
//! what is given and done is the same (`App::body_versions`, `App::body_version`,
//! `App::keep_body_version_now`, `App::restore_body_version`); only the routes are a
//! course's, under the handle that reaches courses (`InCourse`), so that no operation of
//! a card's or a page's versions reaches a unit. Chapters have no body and answer 404.

use fvoci_model::{BodyVersion, BodyVersionContent, KeptBodyVersion, Unit};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::body_versions::{
    body_version_of, body_versions_of, keep_body_version_of, restore_body_version_of,
};
use super::contract::params::{CourseId, MemberWorkspaceId, UnitId, UnitVersionNumber};
use super::contract::tag;
use super::items::{ItemOf, UnitItem};
use super::{ApiResult, Delegated, Read};
use crate::app::App;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(
            handle_get_unit_body_versions,
            handle_keep_unit_body_version
        ))
        .routes(routes!(handle_get_unit_body_version))
        .routes(routes!(handle_restore_unit_body_version))
}

/// The versions kept of a unit's body
///
/// As `getPageBodyVersions` gives a page's: the newest first, each with when it was kept
/// and who wrote the body up to it, without the bodies (`getUnitBodyVersion`). For who
/// reads the course, whatever their role: its members, and of an open course whoever
/// works in the workspace. A chapter or a unit in the trash is not there (404); a unit's
/// versions are kept while it is in the trash, and go when it is gone for good.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/body-versions",
    operation_id = "getUnitBodyVersions",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId),
    responses((status = 200, description = "The versions, the newest first.", body = Vec<BodyVersion>)),
)]
async fn handle_get_unit_body_versions(
    Delegated(unit, ..): Delegated<Read, ItemOf<UnitItem>>,
) -> ApiResult {
    body_versions_of(unit).await
}

/// Keep a unit's body as a version now
///
/// As `keepPageBodyVersion` keeps a page's: the body the unit has, with what those who
/// edit it together typed, by hand; nothing of a body that is its newest version already
/// (`unchanged`), nor within a minute of a version kept by hand (`recent`). For editors
/// of the course (403 for its other members and for who reads an open course unjoined).
/// A chapter or a unit in the trash is not there (404).
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/body-versions",
    operation_id = "keepUnitBodyVersion",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId),
    responses((status = 200, description = "What became of the body.", body = KeptBodyVersion)),
)]
async fn handle_keep_unit_body_version(unit: ItemOf<UnitItem>) -> ApiResult {
    keep_body_version_of(unit).await
}

/// A version kept of a unit's body
///
/// The version with its body, as `getUnit` gives a body. For whom `getUnitBodyVersions`
/// is for. A chapter, or a number the unit has no version of (or has none of any more),
/// is 404.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/body-versions/{version}",
    operation_id = "getUnitBodyVersion",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId, UnitVersionNumber),
    responses((status = 200, description = "The version.", body = BodyVersionContent)),
)]
async fn handle_get_unit_body_version(
    Delegated(unit, ..): Delegated<Read, ItemOf<UnitItem>>,
) -> ApiResult {
    body_version_of(unit).await
}

/// Go back to a version of a unit's body
///
/// Writes the body of the version to the unit again, as `restorePageBodyVersion` does to
/// a page: those who edit the unit together are sent the body as a new document, the
/// body it replaces is kept as a version first, and the body written is the unit's
/// newest version, by the caller. A unit that has that body already is left as it is. For
/// editors of the course (403 for its other members). A chapter, or a number the unit
/// has no version of (or has none of any more), is 404; a version whose body may not be
/// stored any more, or names a file (an image, a file block) the course no longer has,
/// is 409.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/courses/{courseId}/units/{unitId}/body-versions/{version}/restore",
    operation_id = "restoreUnitBodyVersion",
    tag = tag::COURSES,
    params(MemberWorkspaceId, CourseId, UnitId, UnitVersionNumber),
    responses((status = 200, description = "The unit as it is now.", body = Unit)),
)]
async fn handle_restore_unit_body_version(unit: ItemOf<UnitItem>) -> ApiResult {
    restore_body_version_of(unit).await
}
