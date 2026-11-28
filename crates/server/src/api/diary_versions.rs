//! The versions of an entry's body: listing them, reading one, and going back to one. An
//! entry's body is kept as a card's and a page's are, and these are the same operations
//! under the entry's own address.

use fvoci_model::{BodyVersion, BodyVersionContent, DiaryEntry, KeptBodyVersion};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::ApiResult;
use super::body_versions::{
    body_version_of, body_versions_of, keep_body_version_of, restore_body_version_of,
};
use super::contract::params::{DiaryId, EntryId, EntryVersionNumber, OwnWorkspaceId};
use super::contract::tag;
use super::items::{EntryItem, ItemOf};
use crate::app::App;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(
            handle_get_diary_entry_body_versions,
            handle_keep_diary_entry_body_version
        ))
        .routes(routes!(handle_get_diary_entry_body_version))
        .routes(routes!(handle_restore_diary_entry_body_version))
}

/// The versions of an entry's body
///
/// The bodies the entry had before it was written anew, the newest first, as
/// `getCardBodyVersions` lists a card's: the server keeps the newest of them.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/diaries/{diaryId}/entries/{entryId}/body-versions",
    operation_id = "getDiaryEntryBodyVersions",
    tag = tag::DIARIES,
    params(OwnWorkspaceId, DiaryId, EntryId),
    responses((status = 200, description = "The versions, the newest first.", body = Vec<BodyVersion>)),
)]
async fn handle_get_diary_entry_body_versions(entry: ItemOf<EntryItem>) -> ApiResult {
    body_versions_of(entry).await
}

/// Keep an entry's body as a version now
///
/// As `keepCardBodyVersion` keeps a card's: the body the entry has, with what those who
/// edit it together typed, by hand; nothing of a body that is its newest version already
/// (`unchanged`), nor within a minute of a version kept by hand (`recent`). An entry in
/// the trash is not there (404).
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/diaries/{diaryId}/entries/{entryId}/body-versions",
    operation_id = "keepDiaryEntryBodyVersion",
    tag = tag::DIARIES,
    params(OwnWorkspaceId, DiaryId, EntryId),
    responses((status = 200, description = "What became of the body.", body = KeptBodyVersion)),
)]
async fn handle_keep_diary_entry_body_version(entry: ItemOf<EntryItem>) -> ApiResult {
    keep_body_version_of(entry).await
}

/// A version of an entry's body
///
/// The body as it was in the version, as `getCardBodyVersion` answers a card's. 404 for
/// a version the entry does not have.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/diaries/{diaryId}/entries/{entryId}/body-versions/{version}",
    operation_id = "getDiaryEntryBodyVersion",
    tag = tag::DIARIES,
    params(OwnWorkspaceId, DiaryId, EntryId, EntryVersionNumber),
    responses((status = 200, description = "The version.", body = BodyVersionContent)),
)]
async fn handle_get_diary_entry_body_version(entry: ItemOf<EntryItem>) -> ApiResult {
    body_version_of(entry).await
}

/// Go back to a version of an entry's body
///
/// The body of the entry becomes what it was in the version, as
/// `restoreCardBodyVersion` does for a card: what it was until now is kept as a version
/// too, and whoever is editing the body is told of the new one.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/diaries/{diaryId}/entries/{entryId}/body-versions/{version}/restore",
    operation_id = "restoreDiaryEntryBodyVersion",
    tag = tag::DIARIES,
    params(OwnWorkspaceId, DiaryId, EntryId, EntryVersionNumber),
    responses((status = 200, description = "The entry as it is now.", body = DiaryEntry)),
)]
async fn handle_restore_diary_entry_body_version(entry: ItemOf<EntryItem>) -> ApiResult {
    restore_body_version_of(entry).await
}
