//! File upload and download, including byte-range and conditional request handling.

use axum::body::Body;
use std::io;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::SystemTime;

use axum::extract::multipart::{Field, MultipartRejection};
use axum::extract::{Multipart, Path};
use axum::http::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE,
    IF_MODIFIED_SINCE, IF_NONE_MATCH, IF_RANGE, IF_UNMODIFIED_SINCE, LAST_MODIFIED, RANGE,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use futures_util::stream;
use fvoci_model::{FileRecord, Problem};
use serde::Serialize;
use tokio_util::io::StreamReader;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::bodies::{Binary, FileForm};
use super::contract::params::{ContainerId, FileId, MemberWorkspaceId};
use super::contract::tag;
use super::limits;
use super::workspace::InContainer;
use super::{Delegated, Read, Write};

use super::{ApiResult, created, json_response, require_container_permission, require_granted};
use crate::app::App;
use crate::app::{FILE, MAX_UPLOAD_NAME_CHARS, check_file_name};
use crate::error::{AppError, Reason};
use crate::permissions::ContainerPermission;

/// Content types that must not be served as themselves, because a browser would run
/// them; they are served as plain text instead.
const UNSAFE_CONTENT_TYPES: [&str; 6] = [
    "application/javascript",
    "application/ecmascript",
    "text/javascript",
    "text/ecmascript",
    "application/x-javascript",
    "text/html",
];

/// Content types a browser can show inline; anything else is offered as a download.
const MEDIA_CONTENT_TYPES: [&str; 10] = [
    "image/jpeg",
    "image/png",
    "image/bmp",
    "image/gif",
    "image/tiff",
    "video/avi",
    "video/mpeg",
    "video/mp4",
    "audio/mpeg",
    "audio/wav",
];

/// The refusal of an upload whose body is no multipart form.
pub(super) const NOT_MULTIPART: &str = "the body must be a multipart/form-data form";

/// The multipart form field the uploaded file is sent in.
const UPLOAD_FIELD: &str = "file";

/// The refusal of an upload that carries no part with the file.
const NO_FILE_PART: &str = "the request has no part named `file` with a file name";

/// What an upload answers: the name the file is asked for by.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UploadedFile {
    /// The identifier of the file: what `getFile` and `getFileRecord` take, and what an
    /// attachment block, an image or a file block of a body names in its `fileId`.
    pub(super) file_id: String,
}

/// The routes that read files.
pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_serve_file))
        .routes(routes!(handle_get_file_record))
}

/// The route that takes a file, whose size has a limit of its own.
pub(super) fn upload_routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(handle_upload_file))
}

/// A file
///
/// The content of a file of the board, for the board's members. A file is asked for
/// through its own board only: through another board it is not there.
///
/// Every file is sent as `application/octet-stream` and offered as a download
/// (`Content-Disposition: attachment`) under the name it was uploaded with, which
/// `getFileRecord` has: `filename*` carries that name, and `filename` the `fileId` for a
/// reader that knows no `filename*`. A file whose record has no name, or one of more
/// than 255 characters, is offered under its `fileId`. The answer honours `Range` with one range,
/// unless the request has `If-Range`. It may be cached by the browser of who asked but
/// must be revalidated (`Cache-Control: private, no-cache`), and revalidating saves no
/// download: the `Last-Modified` of an answer is the time of that answer, so a request
/// with the `If-Modified-Since` of an earlier answer gets the whole file again.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/containers/{containerId}/files/{fileId}",
    operation_id = "getFile",
    tag = tag::FILES,
    params(MemberWorkspaceId, ContainerId, FileId),
    responses(
        (status = 200, description = "The file.", body = inline(Binary), content_type = "application/octet-stream"),
        (status = 206, description = "The range of the file that was asked for.", body = inline(Binary), content_type = "application/octet-stream"),
        (status = 304, description = "Only for a request with `If-None-Match: *`, or with an `If-Modified-Since` that is not before the time of the answer. No body."),
        (status = 412, description = "The request has an `If-Unmodified-Since` before the time of the answer.", body = Problem, content_type = "application/problem+json"),
        (status = 416, description = "The range is not in the file, or is no range. A range that is not in the file is answered with a `Content-Range` that tells the file's length.", body = Problem, content_type = "application/problem+json"),
    ),
)]
async fn handle_serve_file(
    Delegated(InContainer(ws), ..): Delegated<Read, InContainer>,
    Path((_, container_id, file_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> ApiResult {
    require_file_reader(&ws.app, &ws.caller.user_id, &container_id).await?;
    serve_container_file(&ws.app, &container_id, &file_id, &headers).await
}

/// Checks that the user may read the files of the board.
pub(super) async fn require_file_reader(
    app: &App,
    user_id: &str,
    container_id: &str,
) -> Result<(), AppError> {
    let grant = app.container_grant(user_id, container_id).await?;
    require_granted(grant, ContainerPermission::View, "access denied to board")
}

/// Sends a file of the board, as `getFile` describes it.
pub(super) async fn serve_container_file(
    app: &App,
    container_id: &str,
    file_id: &str,
    headers: &HeaderMap,
) -> ApiResult {
    // A file is its board's. Through another board it is as missing as a file that
    // does not exist: the answers must not tell the two apart.
    let Some(record) = app.file_record(container_id, file_id).await? else {
        return Err(AppError::not_found(FILE));
    };
    serve_stored(app, &record, &record.path, headers).await
}

/// Answers with the bytes stored at `path`, as the file `record`: its newest version, or
/// an older one. 404 if the storage has nothing there.
pub(super) async fn serve_stored(
    app: &App,
    record: &FileRecord,
    path: &str,
    headers: &HeaderMap,
) -> ApiResult {
    if !app.files().exists(path).await? {
        return Err(AppError::not_found(FILE));
    }
    let size = app.files().size(path).await?.unwrap_or(0);
    let name = DownloadName::of(&record.stored_name, &record.name);
    serve_content(app, name, path, size, "", headers).await
}

/// What a download is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DownloadName<'a> {
    /// The stored name: plain letters, for a reader that knows only `filename`.
    stored: &'a str,
    /// The name the file is saved under by a reader that knows `filename*`.
    saved_as: &'a str,
}

impl<'a> DownloadName<'a> {
    /// The download of the file stored as `stored`, saved under the name it was
    /// `uploaded` with: its last segment, whichever way its directories are parted. A
    /// name that is none (blank, `.` or `..`) leaves the stored name, and so does one
    /// longer than [`MAX_UPLOAD_NAME_CHARS`], whose encoding would be a header longer
    /// than a proxy passes on: no upload takes such a name now, and a record made
    /// before the uploads refused them may hold one.
    fn of(stored: &'a str, uploaded: &'a str) -> Self {
        let name = uploaded.rsplit(['/', '\\']).next().unwrap_or_default();
        let is_name = !name.trim().is_empty()
            && name != "."
            && name != ".."
            && name.chars().count() <= MAX_UPLOAD_NAME_CHARS;
        Self {
            stored,
            saved_as: if is_name { name } else { stored },
        }
    }

    /// The value of `Content-Disposition`. Both names are percent-encoded, so nothing of
    /// a name ends the header or begins another parameter of it.
    fn disposition(self, content_type: &str) -> String {
        let kind = if is_media(content_type) {
            "inline"
        } else {
            "attachment"
        };
        let (stored, saved_as) = (percent_encoded(self.stored), percent_encoded(self.saved_as));
        format!("{kind};filename=\"{stored}\"; filename*=UTF-8''{saved_as}")
    }
}

/// What is known about a file
///
/// The name a file was uploaded under, its size and when it was uploaded. For the
/// board's members.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/containers/{containerId}/files/{fileId}/info",
    operation_id = "getFileRecord",
    tag = tag::FILES,
    params(MemberWorkspaceId, ContainerId, FileId),
    responses((status = 200, description = "What is known about the file.", body = FileRecord)),
)]
async fn handle_get_file_record(
    Delegated(InContainer(ws), ..): Delegated<Read, InContainer>,
    Path((_, container_id, file_id)): Path<(String, String, String)>,
) -> ApiResult {
    require_file_reader(&ws.app, &ws.caller.user_id, &container_id).await?;
    Ok(json_response(
        &container_file_record(&ws.app, &container_id, file_id.as_str()).await?,
    ))
}

/// What is known about a file of the board; 404 for no record, and for the record of
/// another board's file.
pub(super) async fn container_file_record(
    app: &App,
    container_id: &str,
    file_id: &str,
) -> Result<FileRecord, AppError> {
    app.file_record(container_id, file_id)
        .await?
        .ok_or_else(|| AppError::not_found(FILE))
}

/// Upload a file
///
/// Stores a file with the board. The request is a `multipart/form-data` form whose
/// part `file` carries the file and a file name of at most 255 characters, without
/// U+0000, of which what follows its last `/` is kept. A name whose last segment,
/// parted by `/` or `\`, is blank (`dir/`), `.` or `..` is 400, as is one too long or
/// with U+0000. The file is limited to one part of an
/// upload in parts (8 MiB unless the server is configured otherwise), and to the
/// `maxFileBytes` of `getClientConfig` where that is less; a larger one is 413. Where
/// the limit is the part, the refusal has the reason `upload_in_parts`: a file within
/// `maxFileBytes` can be sent with `createUpload`. Where it is `maxFileBytes`, the
/// reason is `file_too_large`; where storing the file would exceed the organization's
/// storage limit, the refusal is 413 with the reason `storage_full`. A file that is refused or cut off is not kept. The `fileId` of the answer ends in the extension of the file name, in lower
/// case, with `.jpeg` written `.jpg`; an extension of more than 17 bytes, or with
/// anything but letters and digits in it, is left off. For editors.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/containers/{containerId}/files",
    operation_id = "uploadFile",
    tag = tag::FILES,
    params(MemberWorkspaceId, ContainerId),
    request_body(content = FileForm, content_type = "multipart/form-data"),
    responses((status = 201, description = "The file is stored.", body = UploadedFile)),
)]
async fn handle_upload_file(
    Delegated(InContainer(ws), ..): Delegated<Write, InContainer>,
    Path((_, container_id)): Path<(String, String)>,
    headers: HeaderMap,
    multipart: Result<Multipart, MultipartRejection>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &container_id,
        ContainerPermission::ManageCards,
        "access denied to make board changes",
    )
    .await?;
    let limit = SingleUploadLimit::of(&ws.app);
    let mut multipart = multipart.map_err(|_| AppError::bad_request(NOT_MULTIPART))?;
    let declared = headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if declared.is_some_and(|length| length > limit.bytes.saturating_add(FORM_ALLOWANCE)) {
        if !limits::waits_to_send(&headers) {
            limits::discard(&mut multipart, 0).await;
        }
        return Err(limit.refusal());
    }

    let stored = loop {
        let field = tokio::time::timeout(crate::app::PART_IDLE_TIMEOUT, multipart.next_field())
            .await
            .map_err(|_| AppError::bad_request("the upload timed out waiting for data"))?
            .map_err(|error| AppError::bad_request(error.to_string()))?;
        let Some(field) = field else {
            break Err(AppError::bad_request(NO_FILE_PART).with_parameter(UPLOAD_FIELD));
        };
        if field.name() != Some(UPLOAD_FIELD) {
            continue;
        }
        let Some(filename) = field.file_name().map(str::to_owned) else {
            break Err(AppError::bad_request(NO_FILE_PART).with_parameter(UPLOAD_FIELD));
        };
        if let Err(unfit) = check_file_name(&filename) {
            let refusal = format_args!("the file name of the part `file` {unfit}");
            break Err(AppError::bad_request(refusal).with_parameter(UPLOAD_FIELD));
        }
        // The stored name is what a reader asks for the file by; it stays out of the log.
        tracing::debug!(filename, "uploadFile");
        break store_field(&ws.app, &container_id, &filename, field, limit, user_id).await;
    };
    match stored {
        Ok(file_id) => Ok(created(&UploadedFile { file_id })),
        Err(error) => {
            // The client may still be sending; it reads the refusal once it is done.
            limits::discard(&mut multipart, 0).await;
            Err(error)
        }
    }
}

/// What a form may carry besides the file: the boundaries and the part's headers. A
/// request that declares more than the file's limit and this is refused unread.
const FORM_ALLOWANCE: u64 = 64 * 1024;

/// How the reading of the file went, for telling why storing it failed. Kept in an
/// [`AtomicU8`]: the reader the storage takes must be `Send`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reading {
    /// Nothing went wrong with the reading.
    Fine,
    /// The file passed the limit.
    TooLarge,
    /// The form could not be read.
    Unreadable,
}

impl Reading {
    fn load(state: &AtomicU8) -> Self {
        match state.load(Ordering::Relaxed) {
            1 => Self::TooLarge,
            2 => Self::Unreadable,
            _ => Self::Fine,
        }
    }

    fn value(self) -> u8 {
        match self {
            Self::Fine => 0,
            Self::TooLarge => 1,
            Self::Unreadable => 2,
        }
    }

    fn store(self, state: &AtomicU8) {
        state.store(self.value(), Ordering::Relaxed);
    }
}

/// The largest file `uploadFile` takes, and how a larger one is refused.
#[derive(Debug, Clone, Copy)]
struct SingleUploadLimit {
    bytes: u64,
    /// Whether a larger file could be sent in parts.
    in_parts: bool,
}

impl SingleUploadLimit {
    /// One part of an upload in parts, or the largest file
    /// where that is less.
    fn of(app: &App) -> Self {
        let part = app.config().upload_part_bytes;
        let largest = app.limits().largest_file.get();
        if part < largest {
            Self {
                bytes: part,
                in_parts: true,
            }
        } else {
            Self {
                bytes: largest,
                in_parts: false,
            }
        }
    }

    /// The refusal of a larger file: 413, with the reason `upload_in_parts` where the
    /// file could be sent in parts and `file_too_large` where the server stores none
    /// as large.
    fn refusal(self) -> AppError {
        AppError::payload_too_large(self.bytes).with_reason(if self.in_parts {
            Reason::UploadInParts
        } else {
            Reason::FileTooLarge
        })
    }
}

/// Stores the file a form's part carries, as `filename`, if it is within the limit.
async fn store_field(
    app: &App,
    container_id: &str,
    filename: &str,
    field: Field<'_>,
    limit: SingleUploadLimit,
    user_id: &str,
) -> Result<String, AppError> {
    let state = std::sync::Arc::new(AtomicU8::new(Reading::Fine.value()));
    let state_in_stream = std::sync::Arc::clone(&state);
    let bytes = stream::unfold((field, 0_u64), move |(mut field, mut received)| {
        let state = std::sync::Arc::clone(&state_in_stream);
        async move {
            let next =
                match tokio::time::timeout(crate::app::PART_IDLE_TIMEOUT, field.chunk()).await {
                    Ok(Ok(Some(chunk))) => chunk,
                    Ok(Ok(None)) => return None,
                    Ok(Err(error)) => {
                        Reading::Unreadable.store(&state);
                        return Some((Err(io::Error::other(error)), (field, received)));
                    }
                    Err(_) => {
                        Reading::Unreadable.store(&state);
                        return Some((
                            Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "the upload timed out waiting for data",
                            )),
                            (field, received),
                        ));
                    }
                };
            received = received.saturating_add(u64::try_from(next.len()).unwrap_or(u64::MAX));
            if received > limit.bytes {
                Reading::TooLarge.store(&state);
                return Some((
                    Err(io::Error::other("the file is larger than the limit")),
                    (field, received),
                ));
            }
            Some((Ok(next), (field, received)))
        }
    });
    let mut reader = StreamReader::new(Box::pin(bytes));
    // What fails is not kept: the storage writes a file whole or not at all.
    let saved = app
        .save_file_by(&mut reader, container_id, filename, user_id)
        .await;
    match (saved, Reading::load(&state)) {
        (Ok(file_id), _) => Ok(file_id),
        (Err(_), Reading::TooLarge) => Err(limit.refusal()),
        (Err(_), Reading::Unreadable) => Err(AppError::bad_request("the file could not be read")),
        (Err(error), Reading::Fine) => Err(error),
    }
}

/// A refused form, read field by field to be thrown away.
impl limits::Leftover for &mut Multipart {
    async fn next_piece(&mut self, allowance: u64) -> Option<u64> {
        let mut field = self.next_field().await.ok()??;
        let mut read: u64 = 0;
        while read <= allowance
            && let Ok(Some(chunk)) = field.chunk().await
        {
            read = read.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        }
        Some(read)
    }
}

/// Serves a file's contents with the download headers, answering a range request and a
/// conditional one (RFC 9110 §14 and §13).
async fn serve_content(
    app: &App,
    name: DownloadName<'_>,
    path: &str,
    size: u64,
    mime_type: &str,
    request_headers: &HeaderMap,
) -> ApiResult {
    let modtime = SystemTime::now();
    let content_type = content_type_for(mime_type);
    let mut headers = base_headers(name, &content_type, modtime);

    // Preconditions, in the order RFC 9110 §13.2.2 evaluates them.
    if let Some(since) = http_date(request_headers, &IF_UNMODIFIED_SINCE)
        && modtime_after(modtime, since)
    {
        return Err(AppError::precondition_failed(
            "the file is newer than the If-Unmodified-Since of the request",
        )
        .with_header("If-Unmodified-Since"));
    }
    if if_none_match_star(request_headers) || not_modified(request_headers, modtime) {
        headers.remove(CONTENT_TYPE);
        return Ok((StatusCode::NOT_MODIFIED, headers, Body::empty()).into_response());
    }

    headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));

    let range_header = request_headers
        .get(RANGE)
        .and_then(|value| value.to_str().ok())
        .filter(|_| request_headers.get(IF_RANGE).is_none());
    match range_header.map(|header| parse_range(header, size)) {
        None | Some(RangeResult::Full) => {
            headers.insert(CONTENT_LENGTH, header_value(&size.to_string()));
            let stream = app.files().read(path, None).await?;
            Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response())
        }
        Some(RangeResult::Satisfiable { start, end }) => {
            let length = end - start + 1;
            headers.insert(CONTENT_LENGTH, header_value(&length.to_string()));
            headers.insert(
                CONTENT_RANGE,
                header_value(&format!("bytes {start}-{end}/{size}")),
            );
            let stream = app.files().read(path, Some(start..end + 1)).await?;
            Ok((
                StatusCode::PARTIAL_CONTENT,
                headers,
                Body::from_stream(stream),
            )
                .into_response())
        }
        Some(RangeResult::NotSatisfiable) => {
            let mut response = range_error("the range is not in the file").into_response();
            // What the range could have been: the length of the file.
            response
                .headers_mut()
                .insert(CONTENT_RANGE, header_value(&format!("bytes */{size}")));
            Ok(response)
        }
        Some(RangeResult::Invalid) => Err(range_error("the Range header is no range of bytes")),
    }
}

/// The 416 for a `Range` the file cannot be served by.
fn range_error(message: &str) -> AppError {
    AppError::range_not_satisfiable(message).with_header("Range")
}

/// The headers every download carries before range and conditional handling.
fn base_headers(name: DownloadName<'_>, content_type: &str, modtime: SystemTime) -> HeaderMap {
    let disposition = name.disposition(content_type);
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(CONTENT_TYPE, header_value(content_type));
    headers.insert(CONTENT_DISPOSITION, header_value(&disposition));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    // A file can be asked for with a plain link or an image tag, so a browser may open
    // it as a document of this origin. `sandbox` takes that document's scripts and its
    // origin away: an uploaded SVG or HTML file cannot act as the user.
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("sandbox; frame-ancestors 'none'"),
    );
    headers.insert(
        LAST_MODIFIED,
        header_value(&httpdate::fmt_http_date(modtime)),
    );
    headers
}

/// The content type to serve: octet-stream when unknown, plain text for an unsafe type,
/// otherwise the type itself.
fn content_type_for(mime_type: &str) -> String {
    if mime_type.is_empty() {
        return "application/octet-stream".to_owned();
    }
    if UNSAFE_CONTENT_TYPES
        .iter()
        .any(|unsafe_type| mime_type.starts_with(unsafe_type))
    {
        return "text/plain".to_owned();
    }
    mime_type.to_owned()
}

/// Whether a content type is one a browser shows inline.
fn is_media(content_type: &str) -> bool {
    MEDIA_CONTENT_TYPES
        .iter()
        .any(|media| content_type.starts_with(media))
}

/// The result of parsing a `Range` header against a file of a given size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeResult {
    /// Serve the whole file (no byte range, or more than one range).
    Full,
    /// Serve bytes `start..=end`.
    Satisfiable { start: u64, end: u64 },
    /// A well-formed range that lies outside the file: 416 with a `Content-Range`.
    NotSatisfiable,
    /// A malformed range header: 416 without a `Content-Range`.
    Invalid,
}

/// Parses a `Range` header for a file of `size` bytes. Only a single byte range is
/// served; requests naming more than one range receive the whole file.
fn parse_range(header: &str, size: u64) -> RangeResult {
    let Some(set) = header.strip_prefix("bytes=") else {
        return RangeResult::Invalid;
    };
    let specs: Vec<&str> = set
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if specs.is_empty() {
        return RangeResult::Invalid;
    }
    if specs.len() > 1 {
        return RangeResult::Full;
    }
    // An empty file has no byte a range could name; serve the whole (empty) file rather
    // than 416 because some clients send a Range header on every download request.
    if size == 0 {
        return RangeResult::Full;
    }

    let spec = specs[0];
    let Some((start_text, end_text)) = spec.split_once('-') else {
        return RangeResult::Invalid;
    };

    if start_text.is_empty() {
        // A suffix range: the last N bytes.
        let Ok(suffix) = end_text.parse::<u64>() else {
            return RangeResult::Invalid;
        };
        if suffix == 0 {
            return RangeResult::NotSatisfiable;
        }
        let start = size.saturating_sub(suffix);
        return satisfiable(start, size - 1, size);
    }

    let Ok(start) = start_text.parse::<u64>() else {
        return RangeResult::Invalid;
    };
    if start >= size {
        return RangeResult::NotSatisfiable;
    }
    let end = if end_text.is_empty() {
        size - 1
    } else {
        match end_text.parse::<u64>() {
            Ok(end) => end.min(size - 1),
            Err(_) => return RangeResult::Invalid,
        }
    };
    if end < start {
        return RangeResult::Invalid;
    }
    satisfiable(start, end, size)
}

/// A satisfiable range, guarding against an empty file.
fn satisfiable(start: u64, end: u64, size: u64) -> RangeResult {
    if size == 0 {
        RangeResult::NotSatisfiable
    } else {
        RangeResult::Satisfiable { start, end }
    }
}

/// Whether an `If-None-Match: *` is present, which a GET answers with 304.
fn if_none_match_star(headers: &HeaderMap) -> bool {
    headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim() == "*")
}

/// Whether the file counts as not modified since the request's `If-Modified-Since`.
fn not_modified(headers: &HeaderMap, modtime: SystemTime) -> bool {
    if headers.contains_key(IF_NONE_MATCH) {
        return false;
    }
    match http_date(headers, &IF_MODIFIED_SINCE) {
        Some(since) => !modtime_after(modtime, since),
        None => false,
    }
}

/// Parses an HTTP-date request header.
fn http_date(headers: &HeaderMap, name: &axum::http::HeaderName) -> Option<SystemTime> {
    let text = headers.get(name)?.to_str().ok()?;
    httpdate::parse_http_date(text).ok()
}

/// Whether `modtime`, truncated to seconds, is strictly after `other`.
fn modtime_after(modtime: SystemTime, other: SystemTime) -> bool {
    truncate_to_seconds(modtime) > truncate_to_seconds(other)
}

/// Truncates a time to whole seconds since the epoch, as HTTP-date comparisons do.
fn truncate_to_seconds(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Builds a header value from a string, falling back to empty on an impossible value.
fn header_value(text: &str) -> HeaderValue {
    HeaderValue::from_str(text).unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// Percent-encodes a name for `Content-Disposition`: every byte of its UTF-8 but letters,
/// digits and `-`, `_`, `.`, `~`, which is what the `filename*` of RFC 8187 takes as it
/// is and leaves a quoted `filename` nothing to end it with.
fn percent_encoded(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for &byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            escaped.push(char::from(byte));
        } else {
            escaped.push('%');
            escaped.push(hex_digit(byte >> 4));
            escaped.push(hex_digit(byte & 0x0f));
        }
    }
    escaped
}

/// The upper-case hexadecimal digit of a nibble.
fn hex_digit(nibble: u8) -> char {
    char::from_digit(u32::from(nibble), 16).map_or('0', |digit| digit.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_range_single_range() {
        assert_eq!(
            parse_range("bytes=0-7", 33),
            RangeResult::Satisfiable { start: 0, end: 7 },
            "a leading range"
        );
    }

    #[test]
    fn parse_range_past_the_end_is_not_satisfiable() {
        assert_eq!(
            parse_range("bytes=1000-2000", 33),
            RangeResult::NotSatisfiable,
            "a range past the end"
        );
    }

    #[test]
    fn parse_range_suffix_and_open_ends() {
        assert_eq!(
            parse_range("bytes=-5", 33),
            RangeResult::Satisfiable { start: 28, end: 32 },
            "the last five bytes"
        );
        assert_eq!(
            parse_range("bytes=10-", 33),
            RangeResult::Satisfiable { start: 10, end: 32 },
            "from an offset to the end"
        );
    }

    #[test]
    fn parse_range_clamps_the_end() {
        assert_eq!(
            parse_range("bytes=0-99999", 33),
            RangeResult::Satisfiable { start: 0, end: 32 },
            "an end past the file is clamped"
        );
    }

    #[test]
    fn parse_range_rejects_a_bad_unit_and_serves_multiple_whole() {
        assert_eq!(
            parse_range("lines=0-1", 33),
            RangeResult::Invalid,
            "a bad unit"
        );
        assert_eq!(
            parse_range("bytes=0-1,4-5", 33),
            RangeResult::Full,
            "more than one range serves the whole file"
        );
    }

    #[test]
    fn a_name_is_percent_encoded_but_for_plain_letters() {
        assert_eq!(percent_encoded("My Pic.PNG"), "My%20Pic.PNG", "a space");
        assert_eq!(percent_encoded("plain-1_2~.png"), "plain-1_2~.png", "plain");
        assert_eq!(percent_encoded(r#"a"b.png"#), "a%22b.png", "a quote");
        assert_eq!(
            percent_encoded("보고서.pdf"),
            "%EB%B3%B4%EA%B3%A0%EC%84%9C.pdf",
            "the bytes of UTF-8"
        );
        // What `filename*` does not take as it is, and what would end a parameter.
        assert_eq!(
            percent_encoded("a:b=c@d;e'f*g%h\r\n"),
            "a%3Ab%3Dc%40d%3Be%27f%2Ag%25h%0D%0A"
        );
    }

    #[test]
    fn a_download_is_saved_under_the_name_it_was_uploaded_with() {
        let stored = "7a.pdf";
        let disposition = |uploaded: &str| DownloadName::of(stored, uploaded).disposition("");
        assert_eq!(
            disposition("보고서 1.pdf"),
            "attachment;filename=\"7a.pdf\"; \
             filename*=UTF-8''%EB%B3%B4%EA%B3%A0%EC%84%9C%201.pdf"
        );
        // Nothing of a name ends the header, a parameter of it or its quotes.
        let hostile = disposition("a\"; filename*=UTF-8''evil.sh\r\nSet-Cookie: x=1");
        assert!(
            hostile
                .bytes()
                .all(|byte| byte.is_ascii_graphic() || byte == b' '),
            "{hostile}"
        );
        assert_eq!(hostile.matches('"').count(), 2, "{hostile}");
        assert_eq!(hostile.matches(';').count(), 2, "{hostile}");
        assert_eq!(hostile.matches("filename*=").count(), 1, "{hostile}");
        // Only the last segment is a name, whichever way the directories are parted.
        for (uploaded, saved_as) in [
            ("../../etc/passwd", "passwd"),
            (r"..\..\boot.ini", "boot.ini"),
            (r"c:\dir/x.png", "x.png"),
        ] {
            assert_eq!(
                DownloadName::of(stored, uploaded).saved_as,
                saved_as,
                "{uploaded}"
            );
        }
        // A record without a name, or with one that is none, leaves the stored name.
        for uploaded in ["", "  ", ".", "..", "dir/", r"dir\", "a/.."] {
            assert_eq!(
                disposition(uploaded),
                "attachment;filename=\"7a.pdf\"; filename*=UTF-8''7a.pdf",
                "{uploaded:?}"
            );
        }
        // The longest name an upload in parts takes is the longest a download is saved under.
        let longest = "가".repeat(MAX_UPLOAD_NAME_CHARS);
        assert_eq!(DownloadName::of(stored, &longest).saved_as, longest);
        assert!(
            disposition(&longest).len() < 2400,
            "a header a proxy passes on"
        );
        let longer = "가".repeat(MAX_UPLOAD_NAME_CHARS + 1);
        assert_eq!(DownloadName::of(stored, &longer).saved_as, stored);
        assert!(
            DownloadName::of(stored, "a.png")
                .disposition("image/png")
                .starts_with("inline;"),
            "what a browser shows is shown"
        );
    }

    #[test]
    fn content_type_rules() {
        assert_eq!(
            content_type_for(""),
            "application/octet-stream",
            "unknown is octet-stream"
        );
        assert_eq!(
            content_type_for("text/html; charset=utf-8"),
            "text/plain",
            "an unsafe type is plain text"
        );
        assert_eq!(
            content_type_for("image/png"),
            "image/png",
            "a safe type is kept"
        );
    }
}
