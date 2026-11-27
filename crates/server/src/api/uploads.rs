//! Uploads in parts: a file of up to `max_file_bytes` sent in
//! parts the server sizes, each checked against its SHA-256, and resumed where it
//! stopped.

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::header::CONTENT_LENGTH;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fvoci_model::{Upload, UploadPart, UploadPurpose};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::archive::require_import_access;
use super::contract::bodies::Binary;
use super::contract::params::{
    ContainerId, ContentDigest, MemberWorkspaceId, PartNumber, UploadId, WorkspaceId,
};
use super::contract::tag;
use super::files::UploadedFile;
use super::limits::discard;
use super::workspace::{InContainer, InWorkspace};
use super::{
    ApiResult, RequiredSession, body::decode_object, created, json_response, no_content,
    require_container_permission,
};
use super::{Delegated, Write};
use crate::app::{App, FileToUpload, PartDigest};
use crate::auth::api_token;
use crate::error::AppError;
use crate::permissions::ContainerPermission;

/// What is refused to a user who may not upload files to the board.
const DENIAL: &str = "access denied to make board changes";

/// The header that carries the digest of a part (RFC 9530).
const CONTENT_DIGEST: &str = "content-digest";

/// A file to send in parts.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NewUpload {
    /// The name of the file, as `uploadFile` takes one: 1 to 255 characters, without
    /// U+0000, of which what follows its last `/` is kept. A name whose last segment,
    /// parted by `/` or `\`, is blank (`dir/`), `.` or `..` is refused. The stored file
    /// keeps its extension, as `uploadFile` keeps it.
    name: String,
    /// The size of the file in bytes. More than `maxFileBytes` of `getClientConfig` is
    /// 413 (`file_too_large`).
    size: i64,
    /// The content type of the file, at most 255 characters. The server does not go by
    /// it: what a file is served as follows from its name.
    #[serde(default)]
    content_type: String,
}

/// An upload that is open.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct UploadOpened {
    /// What the other operations of the upload take. Only the user who opened it may
    /// use it.
    upload_id: String,
    /// The size of every part but the last, which has the rest.
    part_size: i64,
    /// How many parts the file comes in, numbered from 1: none for an empty file.
    part_count: i64,
    /// When the upload ends, in milliseconds since the Unix epoch: 24 hours after it was
    /// opened. An upload that has ended is 404, and is thrown away with its parts.
    expires_at: i64,
}

/// An upload that is open, with what it has received.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct UploadState {
    /// The identifier of the upload.
    upload_id: String,
    /// What the upload is for: `file`, a file of a board (`completeUpload`), or
    /// `import`, an archive to import into a workspace (`importArchive`).
    #[schema(inline)]
    purpose: Purpose,
    /// The board the file is for; empty for an archive.
    container_id: String,
    /// The workspace the archive is for; empty for a file.
    workspace_id: String,
    /// The name of the file.
    name: String,
    /// The size of the file in bytes.
    size: i64,
    /// The size of every part but the last.
    part_size: i64,
    /// How many parts the file comes in.
    part_count: i64,
    /// When the upload ends, in milliseconds since the Unix epoch.
    expires_at: i64,
    /// The parts received, by number. A part sent again replaces the one received.
    parts: Vec<ReceivedPart>,
}

/// What an upload is for.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
enum Purpose {
    File,
    Import,
}

/// A part that was received.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ReceivedPart {
    /// The number of the part, from 1.
    number: i64,
    /// Its size in bytes.
    size: i64,
}

impl UploadOpened {
    fn new(upload: &Upload) -> Self {
        Self {
            upload_id: upload.id.clone(),
            part_size: upload.part_size,
            part_count: upload.part_count(),
            expires_at: upload.expires_at,
        }
    }
}

impl UploadState {
    fn new(upload: Upload, parts: &[UploadPart]) -> Self {
        let part_count = upload.part_count();
        Self {
            upload_id: upload.id,
            purpose: match upload.purpose {
                UploadPurpose::File => Purpose::File,
                UploadPurpose::Import => Purpose::Import,
            },
            container_id: upload.container_id,
            workspace_id: upload.workspace_id,
            name: upload.name,
            size: upload.size,
            part_size: upload.part_size,
            part_count,
            expires_at: upload.expires_at,
            parts: parts
                .iter()
                .map(|part| ReceivedPart {
                    number: part.number,
                    size: part.size,
                })
                .collect(),
        }
    }
}

/// The routes that take JSON, or nothing.
pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_create_upload))
        .routes(routes!(handle_create_import_upload))
        .routes(routes!(handle_get_upload, handle_delete_upload))
        .routes(routes!(handle_complete_upload))
}

/// The route that takes the bytes of a part, whose size the upload bounds.
pub(super) fn part_routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(handle_put_upload_part))
}

/// Open an upload
///
/// Opens an upload of a file to the board in parts. The answer says how large the
/// parts are and how many; send each with `putUploadPart`, in any order and again
/// after a failure, then `completeUpload`. A file larger than `maxFileBytes` of
/// `getClientConfig` is 413 with the reason `file_too_large`, and one that would exceed
/// the organization's storage limit is 413 with the reason `storage_full`. A user has at most 8
/// uploads open, holding back at most twice `maxFileBytes` together, and the server's
/// open uploads hold back at most what it is configured for: an upload over one of
/// these is 429, with the reason `too_many_uploads`, `uploads_pending` or
/// `server_uploads_pending`. An upload is open until it is completed, abandoned or
/// ends; one that ends is thrown away with its parts within the hour. For editors, as
/// `uploadFile`.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/containers/{containerId}/uploads",
    operation_id = "createUpload",
    tag = tag::FILES,
    params(MemberWorkspaceId, ContainerId),
    request_body = NewUpload,
    responses((status = 201, description = "The upload is open.", body = UploadOpened)),
)]
async fn handle_create_upload(
    Delegated(InContainer(ws), ..): Delegated<Write, InContainer>,
    Path((_, container_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &container_id,
        ContainerPermission::ManageCards,
        DENIAL,
    )
    .await?;
    let NewUpload {
        name,
        size,
        content_type,
    } = decode_object(&body)?;
    let upload = ws
        .app
        .create_upload(
            user_id,
            &container_id,
            FileToUpload {
                name,
                size,
                content_type,
            },
        )
        .await?;
    Ok(created(&UploadOpened::new(&upload)))
}

/// Open an upload of an archive
///
/// Opens an upload of an archive to import into the workspace, sent in parts as a file is
/// with `createUpload`, and imported with `importArchive` once every part is received.
/// An archive larger than `maxImportBytes` of `getClientConfig` is 413 with the reason
/// `archive_too_large`. It counts
/// toward the same limits as the uploads of files, which hold back at most twice the
/// larger of `maxFileBytes` and `maxImportBytes` for a user. For users of the workspace who
/// are not guests, as `importArchive`.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/archive/uploads",
    operation_id = "createImportUpload",
    tag = tag::ARCHIVES,
    params(WorkspaceId),
    request_body = NewUpload,
    responses((status = 201, description = "The upload is open.", body = UploadOpened)),
)]
async fn handle_create_import_upload(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    Path(workspace_id): Path<String>,
    body: Bytes,
) -> ApiResult {
    let ws = require_import_access(&app, session, workspace_id).await?;
    let NewUpload {
        name,
        size,
        content_type,
    } = decode_object(&body)?;
    let upload = ws
        .app
        .create_import_upload(
            &ws.caller.user_id,
            &ws.workspace_id,
            FileToUpload {
                name,
                size,
                content_type,
            },
        )
        .await?;
    Ok(created(&UploadOpened::new(&upload)))
}

/// Get an upload
///
/// The upload and the parts it has received: what a client that lost its place sends
/// next. Another user's upload, and one that has ended, is 404.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/uploads/{uploadId}",
    operation_id = "getUpload",
    tag = tag::FILES,
    params(MemberWorkspaceId, UploadId),
    responses((status = 200, description = "The upload.", body = UploadState)),
)]
async fn handle_get_upload(
    Delegated(InContainer(ws), ..): Delegated<Write, InContainer>,
    Path((_, upload_id)): Path<(String, String)>,
) -> ApiResult {
    let (upload, parts) = upload_of(&ws, &upload_id).await?;
    Ok(json_response(&UploadState::new(upload, &parts)))
}

/// Send a part
///
/// Sends part `partNumber` of the upload as the body, `application/octet-stream`.
/// `Content-Length` is required and must be the part's size: `partSize`, or the rest
/// for the last part; another length is refused with 400 before the body is looked at.
/// What is sent of a refused part is read and thrown away, up to 16 MiB and for up to 5
/// seconds, so that the client reads the answer; a larger or slower body is cut off.
/// (A browser sets `Content-Length` itself.)
/// `Content-Digest` carries the part's SHA-256. A part whose length or digest differs
/// is 400 and is not kept; a part sent again replaces the one received. An upload that
/// is being completed or abandoned is 409.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/uploads/{uploadId}/parts/{partNumber}",
    operation_id = "putUploadPart",
    tag = tag::FILES,
    params(MemberWorkspaceId, UploadId, PartNumber, ContentDigest),
    request_body(content = inline(Binary), content_type = "application/octet-stream"),
    responses((status = 204, description = "The part is received.")),
)]
async fn handle_put_upload_part(
    Delegated(InContainer(ws), ..): Delegated<Write, InContainer>,
    Path((_, upload_id, number)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult {
    let mut stream = body.into_data_stream();
    let received = async {
        let (upload, _) = upload_of(&ws, &upload_id).await?;
        let number: i64 = number.parse().map_err(|_| {
            AppError::bad_request("the part number is a whole number").with_parameter("partNumber")
        })?;
        let length = content_length(&headers)?;
        let digest = content_digest(&headers)?;
        ws.app
            .put_upload_part(&upload, number, length, &digest, &mut stream)
            .await
    }
    .await;
    match received {
        Ok(()) => Ok(no_content()),
        Err(error) => {
            // The client may still be sending; it reads the refusal once it is done.
            discard(stream, 0).await;
            Err(error)
        }
    }
}

/// Complete an upload
///
/// Makes the file of the upload from its parts and ends the upload. Every part must
/// have been received; a part missing is 400, and the upload stays open for it. The
/// answer is the one of `uploadFile`. For editors of the board, at completion as when
/// the upload was opened: a user who may not upload to the board any more is refused,
/// and the upload is thrown away. An upload that is being completed is 409; an upload
/// of an archive is 400, and is imported with `importArchive`.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/uploads/{uploadId}/complete",
    operation_id = "completeUpload",
    tag = tag::FILES,
    params(MemberWorkspaceId, UploadId),
    responses((status = 201, description = "The file is stored.", body = UploadedFile)),
)]
async fn handle_complete_upload(
    Delegated(InContainer(ws), ..): Delegated<Write, InContainer>,
    Path((_, upload_id)): Path<(String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let (upload, _) = upload_of(&ws, &upload_id).await?;
    if upload.purpose != UploadPurpose::File {
        return Err(AppError::bad_request(
            "the upload is of an archive; import it with importArchive",
        )
        .with_parameter("uploadId"));
    }
    if let Err(refusal) = require_container_permission(
        &ws.app,
        user_id,
        &upload.container_id,
        ContainerPermission::ManageCards,
        DENIAL,
    )
    .await
    {
        // The user left the board, or it is gone: the upload cannot be completed any
        // more, and is thrown away. A completion that began before, with
        // the permission, is let finish first: the throwing away waits for the upload's
        // turn, and finds it gone.
        if refusal.is_client_error() {
            ws.app.discard_upload(&upload).await;
        }
        return Err(refusal);
    }
    let file_id = ws.app.complete_upload(&upload).await?;
    Ok(created(&UploadedFile { file_id }))
}

/// Abandon an upload
///
/// Ends the upload and throws its parts away. An upload that is being completed is
/// 409.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/uploads/{uploadId}",
    operation_id = "deleteUpload",
    tag = tag::FILES,
    params(MemberWorkspaceId, UploadId),
    responses((status = 204, description = "The upload is gone.")),
)]
async fn handle_delete_upload(
    Delegated(InContainer(ws), ..): Delegated<Write, InContainer>,
    Path((_, upload_id)): Path<(String, String)>,
) -> ApiResult {
    let (upload, _) = upload_of(&ws, &upload_id).await?;
    ws.app.abandon_upload(&upload).await?;
    Ok(no_content())
}

/// The caller's upload `upload_id` of the workspace, with the parts it received. An
/// upload of an archive is a person's, as its opening and its import are
/// (`createImportUpload`, `importArchive`): a token that names one is refused, as those
/// routes refuse it.
///
/// # Errors
///
/// Returns 403 with the reason `token_forbidden_endpoint` for a token and an upload of
/// an archive, and the errors of `App::workspace_upload`.
async fn upload_of(
    ws: &InWorkspace,
    upload_id: &str,
) -> Result<(Upload, Vec<UploadPart>), AppError> {
    let found = ws
        .app
        .workspace_upload(upload_id, &ws.caller.user_id)
        .await?;
    if found.0.purpose == UploadPurpose::Import && ws.caller.delegation().is_some() {
        return Err(api_token::refused_as_session());
    }
    Ok(found)
}

/// The length the request declares for its body.
fn content_length(headers: &HeaderMap) -> Result<u64, AppError> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            AppError::bad_request("a part needs a Content-Length").with_header("Content-Length")
        })
}

/// The SHA-256 digest of `Content-Digest` (RFC 9530): `sha-256=:<base64>:`, among
/// other algorithms' digests that are not looked at.
fn content_digest(headers: &HeaderMap) -> Result<PartDigest, AppError> {
    let refusal = || {
        AppError::bad_request("a part needs a Content-Digest of sha-256=:<base64>:")
            .with_header("Content-Digest")
    };
    let value = headers
        .get(CONTENT_DIGEST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(refusal)?;
    let encoded = value
        .split(',')
        .find_map(|entry| {
            let (algorithm, digest) = entry.trim().split_once('=')?;
            algorithm
                .trim()
                .eq_ignore_ascii_case("sha-256")
                .then_some(digest.trim())
        })
        .and_then(|digest| digest.strip_prefix(':')?.strip_suffix(':'))
        .ok_or_else(refusal)?;
    let bytes = STANDARD.decode(encoded).map_err(|_| refusal())?;
    PartDigest::try_from(bytes.as_slice()).map_err(|_| refusal())
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn digest_of(value: &str) -> Result<PartDigest, AppError> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_DIGEST, HeaderValue::from_str(value).unwrap());
        content_digest(&headers)
    }

    #[test]
    fn the_sha_256_digest_is_read_among_others() {
        let zeros = STANDARD.encode([0_u8; 32]);
        assert_eq!(digest_of(&format!("sha-256=:{zeros}:")).unwrap(), [0; 32]);
        assert_eq!(
            digest_of(&format!("sha-512=:AAAA:, SHA-256=:{zeros}:")).unwrap(),
            [0; 32]
        );
        assert!(digest_of("sha-512=:AAAA:").is_err(), "no sha-256");
        assert!(digest_of(&format!("sha-256={zeros}")).is_err(), "no colons");
        assert!(digest_of("sha-256=:AAAA:").is_err(), "not 32 bytes");
        assert!(digest_of("sha-256=:!!:").is_err(), "not base64");
        assert!(content_digest(&HeaderMap::new()).is_err(), "no header");
    }
}
