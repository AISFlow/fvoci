//! The versions kept of a file: listing them, reading one, writing a new one from an
//! uploaded file, and going back to one.
//!
//! A file is its board's or its wiki's, and so are its versions: they are read by who
//! reads the board and written by who edits it. No operation of a read token gives
//! them: who reads a board by its link gets the file's newest version (`getSharedFile`)
//! and nothing of the ones before.

use axum::body::Bytes;
use axum::extract::Path;
use axum::http::HeaderMap;
use fvoci_model::{FileVersion, FileVersionBase, NewFileVersion, Problem};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::body::decode_object;
use super::body_versions::version_number;
use super::contract::bodies::Binary;
use super::contract::params::{ContainerId, FileId, FileVersionNumber, MemberWorkspaceId};
use super::contract::tag;
use super::files::{require_file_reader, serve_stored};
use super::workspace::InContainer;
use super::{ApiResult, created, json_response, require_container_permission};
use super::{Delegated, Read};
use crate::app::{App, FILE};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_file_versions, handle_add_file_version))
        .routes(routes!(handle_get_file_version))
        .routes(routes!(handle_restore_file_version))
}

/// The versions kept of a file
///
/// The newest first, each with when it was written, by whom and its size, without the
/// bytes (`getFileVersion`). A file that was never written anew has one version, the
/// upload. The server keeps the newest versions of a file (its setting
/// `max_file_versions`, fifty unless set otherwise), so the numbers may begin above 1.
/// For who reads the board or the wiki; a file of another is 404.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/containers/{containerId}/files/{fileId}/versions",
    operation_id = "getFileVersions",
    tag = tag::FILES,
    params(MemberWorkspaceId, ContainerId, FileId),
    responses((status = 200, description = "The versions, the newest first.", body = Vec<FileVersion>)),
)]
async fn handle_get_file_versions(
    Delegated(InContainer(ws), ..): Delegated<Read, InContainer>,
    Path((_, container_id, file_id)): Path<(String, String, String)>,
) -> ApiResult {
    require_file_reader(&ws.app, &ws.caller.user_id, &container_id).await?;
    let versions = ws.app.file_versions(&container_id, &file_id).await?;
    Ok(json_response(&versions))
}

/// The bytes of a version of a file
///
/// As `getFile` gives the newest version: sent as `application/octet-stream`, offered
/// as a download under the name the file was uploaded with, with one range honoured. For whom
/// `getFileVersions` is for. A number the file has no version of, or has none of any
/// more, is 404.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/containers/{containerId}/files/{fileId}/versions/{version}",
    operation_id = "getFileVersion",
    tag = tag::FILES,
    params(MemberWorkspaceId, ContainerId, FileId, FileVersionNumber),
    responses(
        (status = 200, description = "The bytes of the version.", body = inline(Binary), content_type = "application/octet-stream"),
        (status = 206, description = "The range of them that was asked for.", body = inline(Binary), content_type = "application/octet-stream"),
        (status = 304, description = "As `getFile` answers it. No body."),
        (status = 412, description = "As `getFile` answers it.", body = Problem, content_type = "application/problem+json"),
        (status = 416, description = "As `getFile` answers it.", body = Problem, content_type = "application/problem+json"),
    ),
)]
async fn handle_get_file_version(
    Delegated(InContainer(ws), ..): Delegated<Read, InContainer>,
    Path((_, container_id, file_id, version)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> ApiResult {
    require_file_reader(&ws.app, &ws.caller.user_id, &container_id).await?;
    let Some(record) = ws.app.file_record(&container_id, &file_id).await? else {
        return Err(AppError::not_found(FILE));
    };
    let kept = ws
        .app
        .kept_file_version(&container_id, &file_id, version_number(&version)?)
        .await?;
    serve_stored(&ws.app, &record, &kept.path, &headers).await
}

/// Write a new version of a file
///
/// Makes a file that was uploaded to the same board or wiki (`uploadFile`,
/// `completeUpload`) the new version of the file of the path: its bytes are what
/// `getFile` gives from now on, wherever the file is shown, and the version before
/// stays to be read (`getFileVersion`) and gone back to (`restoreFileVersion`). The
/// uploaded file is no file of its own after: its `fileId` names nothing. The file of
/// the path keeps its `fileId` and the name it was uploaded with. Versions past those
/// the server keeps go, the oldest first.
///
/// `baseVersion` is the version the new one was made from. If the file has a newer
/// one by now, nothing changes: 409 with the reason `file_version_stale`, and the
/// uploaded file is still a file of the board, to be attached on its own.
///
/// The uploaded file is one that no block shows and that has no versions of its own,
/// and its `fileId` ends in the same extension as the file's; any other, and one the
/// board does not have, is 400 at `/fileId`. For editors.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/containers/{containerId}/files/{fileId}/versions",
    operation_id = "addFileVersion",
    tag = tag::FILES,
    params(MemberWorkspaceId, ContainerId, FileId),
    request_body = NewFileVersion,
    responses(
        (status = 201, description = "The new version, the file's newest.", body = FileVersion),
        (status = 409, description = "The file has a newer version than `baseVersion`: `reason` is `file_version_stale`.", body = Problem, content_type = "application/problem+json"),
    ),
)]
async fn handle_add_file_version(
    InContainer(ws): InContainer,
    Path((_, container_id, file_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_file_editor(&ws.app, user_id, &container_id).await?;
    let request: NewFileVersion = decode_object(&body)?;
    let version = ws
        .app
        .add_file_version(
            &container_id,
            &file_id,
            request.base_version,
            &request.file_id,
            user_id,
        )
        .await?;
    tracing::debug!(
        container_id,
        user_id,
        version = version.version,
        "AddFileVersion"
    );
    Ok(created(&version))
}

/// Go back to a version of a file
///
/// Makes the bytes of the version the file's newest version again, as a new version
/// by the caller (`restoredFrom` names the one gone back to): the versions between
/// stay. A file whose newest version is the one asked for is left as it is, and that
/// version is the answer. `baseVersion` is the file's newest version as the caller
/// read it; if the file has a newer one by now, nothing changes: 409 with the reason
/// `file_version_stale`. A number the file has no version of, or has none of any more,
/// is 404. For editors.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/containers/{containerId}/files/{fileId}/versions/{version}/restore",
    operation_id = "restoreFileVersion",
    tag = tag::FILES,
    params(MemberWorkspaceId, ContainerId, FileId, FileVersionNumber),
    request_body = FileVersionBase,
    responses(
        (status = 200, description = "The file's newest version as it is now.", body = FileVersion),
        (status = 409, description = "The file has a newer version than `baseVersion`: `reason` is `file_version_stale`.", body = Problem, content_type = "application/problem+json"),
    ),
)]
async fn handle_restore_file_version(
    InContainer(ws): InContainer,
    Path((_, container_id, file_id, version)): Path<(String, String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_file_editor(&ws.app, user_id, &container_id).await?;
    let request: FileVersionBase = decode_object(&body)?;
    let restored = ws
        .app
        .restore_file_version(
            &container_id,
            &file_id,
            version_number(&version)?,
            request.base_version,
            user_id,
        )
        .await?;
    tracing::debug!(
        container_id,
        user_id,
        version = restored.version,
        "RestoreFileVersion"
    );
    Ok(json_response(&restored))
}

/// Checks that the user may write the files of the board: who edits it.
pub(super) async fn require_file_editor(
    app: &App,
    user_id: &str,
    container_id: &str,
) -> Result<(), crate::error::AppError> {
    require_container_permission(
        app,
        user_id,
        container_id,
        ContainerPermission::ManageCards,
        "access denied to make board changes",
    )
    .await
}
