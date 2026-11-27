//! File uploads and full archive round trips in the founder's own workspace.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use reqwest::multipart::{Form, Part};
use serde_json::json;
use sha2::{Digest as _, Sha256};

use super::super::routes::archive_entries;
use super::{Case, Parameters};

pub(super) async fn exercise(case: &mut Case, board: &str, card: &str) {
    let at = &[("boardId", board)];
    let payload = format!("{} private file", case.label).into_bytes();
    let filename = format!("{}-private.txt", case.label);
    let request = case.request("uploadFile", at).multipart(Form::new().part(
        "file",
        Part::bytes(payload.clone()).file_name(filename.clone()),
    ));
    let uploaded = case.send("uploadFile", request).await.value();
    let file = case.remember(&uploaded, "fileId");
    let attached = case
        .run(
            "createCardAttachment",
            &[("boardId", board), ("cardId", card)],
            json!({"fileId": file}),
        )
        .await
        .value();
    case.remember(&attached, "id");
    case.present("listContainerFiles", at, &filename).await;
    case.present(
        "listCardFiles",
        &[("boardId", board), ("cardId", card)],
        &filename,
    )
    .await;
    let file_at = &[("boardId", board), ("fileId", file.as_str())];
    let received = case.run("getFile", file_at, json!({})).await;
    assert_eq!(
        received.body, payload,
        "the file remains in its organization"
    );
    let info = case
        .present("getFileRecord", file_at, &filename)
        .await
        .value();
    assert_eq!(
        info["size"],
        payload.len(),
        "file metadata matches its contents"
    );
    versions(case, board, &file, &payload).await;
    let payload = format!("{} private multipart file", case.label).into_bytes();
    let upload = case
        .run(
            "createUpload",
            at,
            json!({"name": "parts.txt", "size": payload.len()}),
        )
        .await
        .value();
    let upload = case.remember(&upload, "uploadId");
    let upload_at = &[("uploadId", upload.as_str())];
    let state = case.present("getUpload", upload_at, &upload).await.value();
    assert_eq!(state["containerId"], board, "upload belongs to this board");
    part(case, &upload, &payload).await;
    let completed = case
        .run("completeUpload", upload_at, json!({}))
        .await
        .value();
    let file = case.remember(&completed, "fileId");
    let received = case
        .run(
            "getFile",
            &[("boardId", board), ("fileId", &file)],
            json!({}),
        )
        .await;
    assert_eq!(
        received.body, payload,
        "completed parts remain in their organization"
    );
    let abandoned = case
        .run(
            "createUpload",
            at,
            json!({"name": "abandoned.txt", "size": 0}),
        )
        .await
        .value();
    let abandoned = case.remember(&abandoned, "uploadId");
    case.run("deleteUpload", &[("uploadId", &abandoned)], json!({}))
        .await;
    archives(case, board).await;
}

/// The versions of the file: another upload becomes its second, the first is read and
/// gone back to, so the file is what it was.
async fn versions(case: &mut Case, board: &str, file: &str, first: &[u8]) {
    let at = &[("boardId", board)];
    let file_at = &[("boardId", board), ("fileId", file)];
    case.present("getFileVersions", file_at, "\"version\":1")
        .await;
    let request = case.request("uploadFile", at).multipart(Form::new().part(
        "file",
        Part::bytes(format!("{} private version", case.label).into_bytes()).file_name("next.txt"),
    ));
    let next = case.send("uploadFile", request).await.value();
    let next = case.remember(&next, "fileId");
    case.run(
        "addFileVersion",
        file_at,
        json!({"baseVersion": 1, "fileId": next}),
    )
    .await;
    let first_at = &[("boardId", board), ("fileId", file), ("version", "1")];
    let kept = case.run("getFileVersion", first_at, json!({})).await;
    assert_eq!(
        kept.body, first,
        "an older version remains in its organization"
    );
    case.run("restoreFileVersion", first_at, json!({"baseVersion": 2}))
        .await;
    lease(case, file_at).await;
}

/// The file's lease: the founder takes it, is told it is theirs, and lets it go.
async fn lease(case: &mut Case, file_at: Parameters<'_>) {
    let held = case
        .run("holdFileLease", file_at, json!({"leaseId": ""}))
        .await
        .value();
    let lease = case.remember(&held, "leaseId");
    case.present("getFileLease", file_at, "\"editing\":true")
        .await;
    let lease_at = &[file_at, &[("leaseId", lease.as_str())]].concat();
    case.run("releaseFileLease", lease_at, json!({})).await;
}

async fn part(case: &mut Case, upload: &str, bytes: &[u8]) {
    let digest = format!("sha-256=:{}:", STANDARD.encode(Sha256::digest(bytes)));
    let request = case
        .request(
            "putUploadPart",
            &[("uploadId", upload), ("partNumber", "1")],
        )
        .header("Content-Type", "application/octet-stream")
        .header("Content-Digest", digest)
        .body(bytes.to_vec());
    case.send("putUploadPart", request).await;
}

async fn archives(case: &mut Case, board: &str) {
    let archive = case
        .run("exportContainerArchive", &[("boardId", board)], json!({}))
        .await;
    assert!(
        archive.body.starts_with(b"PK"),
        "a board export is an archive"
    );
    let all = case.run("exportWorkspaceArchive", &[], json!({})).await;
    assert!(
        all.body.starts_with(b"PK"),
        "a workspace export is an archive"
    );
    check_archive(case, &archive.body);
    check_archive(case, &all.body);
    let upload = case
        .run(
            "createImportUpload",
            &[],
            json!({"name": "parity.fvoci", "size": archive.body.len()}),
        )
        .await
        .value();
    let upload = case.remember(&upload, "uploadId");
    part(case, &upload, &archive.body).await;
    case.run("importArchive", &[], json!({"uploadId": upload}))
        .await;
    let boards = case.run("getWorkspaceBoards", &[], json!({})).await.value();
    let copies = boards
        .as_array()
        .expect("boards")
        .iter()
        .filter(|candidate| candidate["title"] == format!("secret {} renamed", case.label))
        .count();
    assert_eq!(copies, 2, "import makes a second copy in this workspace");
}

fn check_archive(case: &Case, bytes: &[u8]) {
    let entries = archive_entries(bytes);
    for (_, contents) in &entries {
        case.assert_is_ours("exported archive", contents);
    }
    assert!(
        entries
            .iter()
            .any(|(_, contents)| String::from_utf8_lossy(contents).contains(&case.label)),
        "archive contains this organization's board"
    );
}
