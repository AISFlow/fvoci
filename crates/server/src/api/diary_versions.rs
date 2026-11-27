//! The versions of an entry's body: listing them, reading one, and going back to one. An
//! entry's body is kept as a card's and a page's are, and these are the same operations
//! under the entry's own address.

use axum::extract::Path;
use fvoci_model::{BodyVersion, BodyVersionContent, DiaryEntry, KeptBodyVersion};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::body_versions::{
    get_body_version, keep_body_version, list_body_versions, restore_body_version,
};
use super::contract::params::{DiaryId, EntryId, EntryVersionNumber, OwnWorkspaceId};
use super::contract::tag;
use super::diaries::{READ_DENIED, entry_with_permission};
use super::workspace::InDiary;
use super::{ApiResult, json_response};
use crate::app::App;
use crate::permissions::ContainerPermission;

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
async fn handle_get_diary_entry_body_versions(
    InDiary(ws): InDiary,
    Path((_, diary_id, entry_id)): Path<(String, String, String)>,
) -> ApiResult {
    let entry = entry_with_permission(
        &ws,
        &diary_id,
        &entry_id,
        ContainerPermission::View,
        READ_DENIED,
    )
    .await?;
    list_body_versions(&ws.app, &entry.id).await
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
async fn handle_keep_diary_entry_body_version(
    InDiary(ws): InDiary,
    Path((_, diary_id, entry_id)): Path<(String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let entry = entry_with_permission(
        &ws,
        &diary_id,
        &entry_id,
        ContainerPermission::ManageCards,
        "access denied to keep a version of the entry's body",
    )
    .await?;
    let kept = keep_body_version(&ws.app, &entry.id).await?;
    tracing::debug!(diary_id, entry_id, user_id, outcome = ?kept.outcome, "KeepDiaryEntryBodyVersion");
    Ok(json_response(&kept))
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
async fn handle_get_diary_entry_body_version(
    InDiary(ws): InDiary,
    Path((_, diary_id, entry_id, version)): Path<(String, String, String, String)>,
) -> ApiResult {
    let entry = entry_with_permission(
        &ws,
        &diary_id,
        &entry_id,
        ContainerPermission::View,
        READ_DENIED,
    )
    .await?;
    get_body_version(&ws.app, &entry.id, &version).await
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
async fn handle_restore_diary_entry_body_version(
    InDiary(ws): InDiary,
    Path((_, diary_id, entry_id, version)): Path<(String, String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let entry = entry_with_permission(
        &ws,
        &diary_id,
        &entry_id,
        ContainerPermission::ManageCards,
        "access denied to restore a version of the entry's body",
    )
    .await?;
    let block = restore_body_version(&ws.app, &entry, &version, user_id).await?;
    tracing::debug!(diary_id, entry_id, user_id, "RestoreDiaryEntryBodyVersion");
    Ok(json_response(&DiaryEntry::from(block)))
}
