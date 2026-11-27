//! Who has a file open in the editor: holding its lease while editing it, letting it
//! go, and telling the board's readers whether somebody edits it.
//!
//! A lease keeps a second editor from starting while one edits, and tells the others who
//! edits; it keeps nobody from writing a version (`addFileVersion` is held to the
//! version it was made from). As the versions, no operation of a read token gives it.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{FileLease, FileLeaseRequest, HeldFileLease, Problem};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::body::decode_object;
use super::contract::params::{ContainerId, FileId, LeaseId, MemberWorkspaceId};
use super::contract::tag;
use super::file_versions::require_file_editor;
use super::files::require_file_reader;
use super::workspace::InContainer;
use super::{ApiResult, json_response, no_content};
use crate::app::App;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_file_lease, handle_hold_file_lease))
        .routes(routes!(handle_release_file_lease))
}

/// Whether somebody edits a file
///
/// Whether somebody holds the file's lease (`holdFileLease`): has it open in the
/// editor. Who, only where the caller sees that person among the people of the
/// workspace, as presence names them, and where it is the caller; else `editorId` is
/// empty and the caller is to say "somebody else". For who reads the board or the wiki.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/containers/{containerId}/files/{fileId}/lease",
    operation_id = "getFileLease",
    tag = tag::FILES,
    params(MemberWorkspaceId, ContainerId, FileId),
    responses((status = 200, description = "Whether somebody edits the file, and who.", body = FileLease)),
)]
async fn handle_get_file_lease(
    InContainer(ws): InContainer,
    Path((_, container_id, file_id)): Path<(String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_file_reader(&ws.app, user_id, &container_id).await?;
    let lease = ws
        .app
        .file_lease(&container_id, &file_id, user_id, &ws.workspace_id, ws.role)
        .await?;
    Ok(json_response(&lease))
}

/// Hold the lease of a file to edit it
///
/// With an empty `leaseId`, takes the file's lease if nobody holds it; with the lease
/// the caller holds, holds on to it. Either way the lease lasts `seconds` from now,
/// and is held on to before then (the web app does so every 30 seconds while the
/// editor is open). A lease that ended and that nobody else took meanwhile is taken
/// anew, under a new `leaseId`. A lease is one editor's: the caller's other window is
/// refused as anybody else is. 409 with the reason `file_being_edited` while another
/// lease of the file is held; `getFileLease` then says who. For editors.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/containers/{containerId}/files/{fileId}/lease",
    operation_id = "holdFileLease",
    tag = tag::FILES,
    params(MemberWorkspaceId, ContainerId, FileId),
    request_body = FileLeaseRequest,
    responses(
        (status = 200, description = "The lease the caller holds now.", body = HeldFileLease),
        (status = 409, description = "Another lease of the file is held: `reason` is `file_being_edited`.", body = Problem, content_type = "application/problem+json"),
    ),
)]
async fn handle_hold_file_lease(
    InContainer(ws): InContainer,
    Path((_, container_id, file_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_file_editor(&ws.app, user_id, &container_id).await?;
    let request: FileLeaseRequest = decode_object(&body)?;
    let held = ws
        .app
        .hold_file_lease(&container_id, &file_id, user_id, &request.lease_id)
        .await?;
    Ok(json_response(&held))
}

/// Let the lease of a file go
///
/// The editor is closed: whoever asks next may edit the file. A lease that is the
/// caller's no more (it ended, and maybe another took the file's), or never was, is
/// left as it is, and the answer is the same. For editors.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/containers/{containerId}/files/{fileId}/lease/{leaseId}",
    operation_id = "releaseFileLease",
    tag = tag::FILES,
    params(MemberWorkspaceId, ContainerId, FileId, LeaseId),
    responses((status = 204, description = "The caller holds no lease of the file.")),
)]
async fn handle_release_file_lease(
    InContainer(ws): InContainer,
    Path((_, container_id, file_id, lease_id)): Path<(String, String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_file_editor(&ws.app, user_id, &container_id).await?;
    ws.app
        .release_file_lease(&container_id, &file_id, user_id, &lease_id)
        .await?;
    Ok(no_content())
}
