//! Exporting and importing archives of boards and wikis.

use axum::Extension;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Path, State};
use axum::response::{IntoResponse, Response};
use fvoci_model::{Container, ContainerKinds, Session, UploadPurpose};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::body::decode_object;
use super::contract::bodies::Binary;
use super::contract::params::{ContainerId, MemberWorkspaceId, WorkspaceId};
use super::contract::tag;
use super::workspace::{InContainer, InWorkspace};
use super::{ApiResult, RequiredSession, json_response};
use crate::app::{App, ExportedArchive, ImportFormat, ImportSummary, Slot, download, utc_day};
use crate::connection::{Breaker, Connection};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

/// An archive or external export to import: sent in parts with `createImportUpload`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportRequest {
    /// The upload of the archive, every part of it received. It is thrown away by the
    /// import, whether the import succeeds or not.
    upload_id: String,
    /// Source format of the uploaded file: `fvoci` (native archive `.fvoci`/`.zip`,
    /// default when omitted), `trello` (Trello board `.json`), or `notion` (Notion
    /// Markdown & CSV `.zip`).
    #[serde(default)]
    format: ImportFormat,
}

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_export_container))
        .routes(routes!(handle_export_container_markdown))
        .routes(routes!(handle_export_workspace))
        .routes(routes!(handle_import))
}

/// Enters the named workspace for a member who may import into it.
///
/// # Errors
///
/// Returns 404 when the user is not a member or is a guest of the workspace.
pub(super) async fn require_import_access(
    app: &App,
    session: Session,
    workspace_id: String,
) -> Result<InWorkspace, AppError> {
    InWorkspace::enter_as(app, session.into(), workspace_id)
        .await?
        .require_no_guest()
}

/// Export a board or a wiki
///
/// An archive of the board with its blocks, its files and its members, offered as a
/// download with its length. For the board's members. The archive is a `.fvoci` ZIP of
/// version 1: `version.json`, and per container a directory with `container.jsonl` (one
/// `container`, `block`, `containerMember` or `courseSchedule` per line, in the shapes
/// of this contract, bodies included, naming their container `containerId`)
/// and the files the blocks show, those of the images and file blocks of bodies among
/// them. A container's line also carries `fileNames`: the name of each of its files, by
/// stored name, which an import gives the files it stores. A name an upload could not
/// have (blank, `.` or `..` after its last `/` or `\`, with U+0000, over 255 characters, or of another extension than the
/// stored name) is passed over and its file keeps the name of its entry, and an archive
/// without the member is read as before. A file a block shows that the board does not
/// have, its record or its bytes gone, gets no entry: it is listed under `missingFiles`
/// of the container's line, by stored name. A wiki is written as a board is, its `kind` `wiki`: its
/// pages as they stand and the comments on them are its blocks (not the pages in its
/// trash, nor the versions of bodies).
///
/// A user runs one export at a time, and the server as many as its setting
/// `max_concurrent_exports` (2 unless set). Over either it is 429 with the reason
/// `export_running` or `server_exports_busy`, and is to be asked for again later. An
/// export runs until its archive is sent or the client goes away; a download that takes
/// nothing for `export_stall_seconds` (60 unless set) is given up, and its connection
/// broken before the archive's end.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/containers/{containerId}/archive/export",
    operation_id = "exportContainerArchive",
    tag = tag::ARCHIVES,
    params(MemberWorkspaceId, ContainerId),
    responses((
        status = 200,
        description = "The archive.",
        body = inline(Binary),
        content_type = "application/octet-stream",
        headers(
            ("Content-Length" = u64, description = "The size of the archive in bytes."),
            ("Content-Disposition" = String, description = "`attachment; filename=archive-YYYY-MM-DD.fvoci`, for the day (UTC)."),
        ),
    )),
)]
async fn handle_export_container(
    InContainer(ws): InContainer,
    connection: Option<Extension<ConnectInfo<Connection>>>,
    Path((_, container_id)): Path<(String, String)>,
) -> ApiResult {
    let container = exported_container(&ws, &container_id).await?;
    let slot = ws.app.start_export(&ws.caller.user_id)?;
    let archive = ws
        .app
        .export_archive(&container.workspace_id, std::slice::from_ref(&container.id))
        .await?;
    Ok(archive_response(
        &ws.app,
        archive,
        slot,
        breaker_of(connection),
        Download::FVOCI,
    ))
}

/// Export a board, a wiki, a course or a diary as Markdown
///
/// A ZIP of Markdown files of the container, for another tool to read, offered as a
/// download with its length, for those `exportContainerArchive` gives the archive to
/// (a diary to its owner alone, as every route of one), and under its limits (one
/// export at a time, `max_concurrent_exports`, `export_stall_seconds`). A session
/// alone: a personal API token is refused.
///
/// The ZIP holds a folder of the container's name, and in it a `.md` file for each
/// page of a wiki (in folders as its tree has them: a page with pages below it has a
/// folder of its name beside its file), each card of a board but its templates
/// (`cards/<number>-<title>.md`, and all of them in `board.csv`), each unit of a course
/// (a chapter is a folder of its units), and each entry of a diary
/// (`entries/<YYYY-MM-DD>.md`); and the files their bodies show and the attachments of
/// the cards in `files/`, named as they were uploaded. A name is one any system takes,
/// and one of its own in its folder (` (2)` after a second). A file starts with its
/// front matter in YAML (`title`, `created`, `updated` in UTC, and a card's `number`
/// and `attachments`, a page's `icon`, a unit's `amount`, `amountUnit` and
/// `scheduledDate`, an entry's `date`, `mood`, `weather` and `place`), then its title
/// as a heading and its body in Markdown as `getCard` writes one with `format=markdown`,
/// but that a file, a page, a card or a unit the ZIP holds is linked where it is (a
/// relative link), a person is named in plain text (`@name`), and what else a reference
/// names is its title where the user reads it, `[[id]]` where not. Comments, the trash
/// and the versions of bodies are not in it.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/containers/{containerId}/markdown/export",
    operation_id = "exportContainerMarkdown",
    tag = tag::ARCHIVES,
    params(MemberWorkspaceId, ContainerId),
    responses((
        status = 200,
        description = "The ZIP of Markdown files.",
        body = inline(Binary),
        content_type = "application/zip",
        headers(
            ("Content-Length" = u64, description = "The size of the ZIP in bytes."),
            ("Content-Disposition" = String, description = "`attachment; filename=markdown-YYYY-MM-DD.zip`, for the day (UTC)."),
        ),
    )),
)]
async fn handle_export_container_markdown(
    InContainer(ws): InContainer,
    connection: Option<Extension<ConnectInfo<Connection>>>,
    Path((_, container_id)): Path<(String, String)>,
) -> ApiResult {
    let container = exported_container(&ws, &container_id).await?;
    let slot = ws.app.start_export(&ws.caller.user_id)?;
    let archive = ws
        .app
        .export_markdown(ws.markdown_reader(), &container)
        .await?;
    Ok(archive_response(
        &ws.app,
        archive,
        slot,
        breaker_of(connection),
        Download::MARKDOWN,
    ))
}

/// The container `container_id` for one who may export it: who reads it and is on it
/// (any reader of a template), and works in its workspace.
///
/// # Errors
///
/// Returns 404 for what the user does not reach, and 403 for what they may not export.
async fn exported_container(ws: &InWorkspace, container_id: &str) -> Result<Container, AppError> {
    let access = ws
        .app
        .container_access(&ws.caller.user_id, container_id)
        .await?
        .ok_or_else(|| AppError::not_found("board"))?;
    super::require_granted(
        access.grant,
        ContainerPermission::View,
        "access denied to board",
    )?;
    if (!access.container.is_template && !access.has_membership()) || !access.actor.works_there() {
        return Err(AppError::forbidden("access denied to export board"));
    }
    Ok(access.container)
}

/// What an export is offered as: the name of its file before the day, its extension,
/// and its type.
#[derive(Debug, Clone, Copy)]
struct Download {
    name: &'static str,
    extension: &'static str,
    content_type: &'static str,
}

impl Download {
    /// The native archive, `archive-YYYY-MM-DD.fvoci`.
    const FVOCI: Self = Self {
        name: "archive",
        extension: "fvoci",
        content_type: "application/octet-stream",
    };
    /// The Markdown files, `markdown-YYYY-MM-DD.zip`.
    const MARKDOWN: Self = Self {
        name: "markdown",
        extension: "zip",
        content_type: "application/zip",
    };
}

/// Export the boards and wikis of a workspace
///
/// An archive of the boards and wikis of the workspace that the user is a member of
/// with a role, and of its open ones for who works in the workspace (also those they
/// have not joined), offered as a download with its length, in the format of
/// `exportContainerArchive`, and under its limits. 404 unless the user is a member of the
/// workspace; a guest's archive has the boards and wikis they were invited to.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/archive/export",
    operation_id = "exportWorkspaceArchive",
    tag = tag::ARCHIVES,
    params(MemberWorkspaceId),
    responses((
        status = 200,
        description = "The archive.",
        body = inline(Binary),
        content_type = "application/octet-stream",
        headers(
            ("Content-Length" = u64, description = "The size of the archive in bytes."),
            ("Content-Disposition" = String, description = "`attachment; filename=archive-YYYY-MM-DD.fvoci`, for the day (UTC)."),
        ),
    )),
)]
async fn handle_export_workspace(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    connection: Option<Extension<ConnectInfo<Connection>>>,
    Path(workspace_id): Path<String>,
) -> ApiResult {
    let ws = InWorkspace::enter_as(&app, session.into(), workspace_id).await?;
    let app = &ws.app.clone().reaching(ContainerKinds::EVERY);
    let user_id = &ws.caller.user_id;
    let containers = app
        .readable_workspace_containers(user_id, &ws.workspace_id, ws.role)
        .await?;
    let ids: Vec<String> = containers
        .into_iter()
        .map(|container| container.id)
        .collect();
    let slot = app.start_export(user_id)?;
    let archive = app.export_archive(&ws.workspace_id, &ids).await?;
    Ok(archive_response(
        app,
        archive,
        slot,
        breaker_of(connection),
        Download::FVOCI,
    ))
}

/// Import an archive
///
/// Makes the boards and wikis of an archive in the workspace, with the user as their
/// admin. The members the archive names for a board become members of it with the roles
/// they had, where they are in the workspace. The archive is sent before in parts: open an
/// upload with `createImportUpload`, send its parts with `putUploadPart`, then name
/// the upload here, in place of `completeUpload`. A part missing is 400 and the upload
/// stays open for it; otherwise the upload ends with the import, whether it succeeds or
/// not. Another user's upload, one that has ended, and one for another workspace are
/// 404; an upload of a file is 400. A workspace the user is not in is 404 before the
/// body is read. For users of the workspace who are not guests, at the import as when
/// the upload was opened: a user who may not import any more is refused, and the upload
/// is thrown away.
///
/// The archive is of version 1, as the export routes write it. Any other version in
/// `version.json`, a non-ZIP payload, and a ZIP without `version.json` are 400, and
/// nothing of them is stored. An archive of more than 10,000 entries, whose entries
/// unpack to more than 1 GiB together, or with a `container.jsonl` of more than 64 MiB
/// (or a single JSONL line over 10 MiB) or a file larger than `maxFileBytes` is 413 with
/// the `limit` passed, and nothing of it is stored; for the entry or line that is too
/// large the reason is `archive_entry_too_large`. The blocks that show files are
/// pointed at the board's new copies of them, the images and file blocks of bodies
/// included. A file the container's line lists under `missingFiles` is not stored, even when
/// the archive has an entry of its name, and what shows it ends up showing no file. A card value that does not fit its property's kind is cleared, as a copy
/// does. A wiki of the archive (`kind` `wiki`) is made as a wiki, its pages and the
/// comments on them with it. What a write would refuse of an archive's boards, wikis
/// and blocks is 400 before any of them is stored, with the message the write has: a
/// block of a type of the other kind (a page on a board, a card on a wiki), a page
/// below anything but a page of its wiki or below a page the archive does not have,
/// pages that are each other's parents, a comment on anything but a card or page, a
/// wiki that is a template, and a wiki deeper or with more pages than the
/// organization's limits allow (400 with the reason `page_too_deep` or
/// `too_many_pages`).
///
/// The containers, blocks, memberships and files of the archive are committed all or
/// none: if any container or file fails, all staged files are removed and nothing is
/// stored. The members of a board who are not in the workspace are left out of it, a
/// guest of the workspace keeps no role above editor; the answer says how many users
/// were left out, not who.
///
/// A user runs one import at a time, and the server as many as its setting
/// `max_concurrent_imports` (2 unless set). Over either it is 429 with the reason
/// `import_running` or `server_imports_busy`; the upload stays open, to be named again.
///
/// Optional `format` selects `"fvoci"` (default, `.fvoci`/`.zip`), `"trello"`
/// (`.json`), or `"notion"` (`.zip`). External formats commit all boards, wikis, blocks,
/// and bundled local files atomically: any validation, limit, or storage failure leaves
/// nothing stored. Returns a structured `ImportSummary` with counts, newly created
/// container identifiers, native `membersLeftOut`, and bounded `skipped` / `warnings`.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/archive/import",
    operation_id = "importArchive",
    tag = tag::ARCHIVES,
    params(WorkspaceId),
    request_body = ImportRequest,
    responses((
        status = 200,
        description = "Summary of the boards, wikis, blocks, and files created by the import.",
        body = ImportSummary,
    )),
)]
async fn handle_import(InContainer(ws): InContainer, body: Bytes) -> ApiResult {
    let ws = ws.require_no_guest()?;
    let user_id = ws.caller.user_id.clone();
    // Membership is checked before reading the body; the upload belongs to this workspace.
    let ImportRequest { upload_id, format } = decode_object(&body)?;
    let (upload, _) = ws.app.workspace_upload(&upload_id, &user_id).await?;
    if upload.purpose != UploadPurpose::Import {
        return Err(AppError::bad_request(
            "the upload is of a file; complete it with completeUpload",
        )
        .with_field("uploadId"));
    }
    // Refused, the upload stays: the import can be asked for again.
    let slot = ws.app.start_import(&user_id)?;
    let app = ws.app.clone();
    // Run inside `spawn_tracked` so a client disconnect mid-import never drops an
    // in-flight S3 multipart upload or leaves staged objects uncleaned.
    let summary = ws
        .app
        .spawn_tracked(async move {
            let _slot = slot;
            app.import_upload_with_format(&upload, format).await
        })
        .await
        .map_err(|error| AppError::internal(format_args!("import task failed: {error}")))?
        .map_err(AppError::outside_the_request)?;
    Ok(json_response(&summary))
}

/// Streams a prepared archive download as `offered`, with its `Content-Length` and
/// attachment headers. The export's `slot` and its file are held until the last piece of
/// the archive is read, the client goes away, or the download stalls (`app::download`).
fn archive_response(
    app: &App,
    archive: ExportedArchive,
    slot: Slot,
    breaker: Option<Breaker>,
    offered: Download,
) -> Response {
    let stall = app.config().export_stall();
    let size = archive.size;
    let filename = export_filename(crate::auth::now_millis(), offered);
    (
        [
            ("content-type", offered.content_type.to_owned()),
            (
                "content-disposition",
                format!("attachment; filename={filename}"),
            ),
            ("content-transfer-encoding", "binary".to_owned()),
            ("content-length", size.to_string()),
        ],
        Body::from_stream(download(archive, slot, stall, cutting(breaker))),
    )
        .into_response()
}

/// What the download calls to break its connection: cuts `breaker`, if there is one.
fn cutting(breaker: Option<Breaker>) -> impl FnOnce() + Send + 'static {
    move || {
        if let Some(breaker) = breaker {
            breaker.cut();
        }
    }
}

/// The breaker of the connection a request came on, if it has one.
fn breaker_of(connection: Option<Extension<ConnectInfo<Connection>>>) -> Option<Breaker> {
    connection.and_then(|Extension(ConnectInfo(connection))| connection.breaker)
}

/// The file name an export is offered under: `archive-YYYY-MM-DD.fvoci` (or
/// `markdown-YYYY-MM-DD.zip`) for today (UTC).
fn export_filename(now_millis: i64, offered: Download) -> String {
    format!(
        "{}-{}.{}",
        offered.name,
        utc_day(now_millis),
        offered.extension
    )
}

#[cfg(test)]
mod tests {
    use super::{Download, cutting, export_filename};
    use crate::connection::Breaker;

    #[test]
    fn the_download_is_handed_what_cuts_the_connection() {
        let breaker = Breaker::default();
        let cut = cutting(Some(breaker.clone()));
        assert!(!breaker.is_cut(), "not before it is called");
        cut();
        assert!(breaker.is_cut(), "the connection's breaker is cut");
        // The administration socket has no breaker: nothing to cut.
        cutting(None)();
    }

    #[test]
    fn export_filename_formats_the_date() {
        // 2026-11-10T05:15:39Z.
        assert_eq!(
            export_filename(1_794_287_739_000, Download::FVOCI),
            "archive-2026-11-10.fvoci",
            "the archive file name for a known date"
        );
        assert_eq!(
            export_filename(1_794_287_739_000, Download::MARKDOWN),
            "markdown-2026-11-10.zip"
        );
    }
}
