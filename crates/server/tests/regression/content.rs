//! Scenarios: templates and the welcome board, files, and archives.
#![expect(
    clippy::too_many_lines,
    reason = "a scenario is one request sequence; cutting it up would hide the order it pins"
)]

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::common::ROOT_WORKSPACE_ID;
use crate::fixtures::{PNG, block, has_extension, id_of, new_blocks, new_board, new_card};
use crate::harness::ROOT_API;
use crate::harness::{API, OCTET_STREAM, Payload, Session, multipart, string_member};

pub(crate) async fn templates(s: &mut Session) {
    let [alice] = s.users("tpl", ["alice"]).await;
    let listing = s
        .get("templates", &format!("{ROOT_API}/templates"))
        .user(&alice)
        .unrecorded()
        .await;
    let listing = listing.json().as_array().expect("the templates are a list");
    let titles: Vec<&str> = listing
        .iter()
        .filter_map(|board| board["title"].as_str())
        .collect();
    s.note("template titles", &json!(titles));
    let mut versions: Vec<i64> = listing
        .iter()
        .filter_map(|board| board["templateVersion"].as_i64())
        .collect();
    versions.sort_unstable();
    versions.dedup();
    s.note("template versions", &json!(versions));
    let mut members: Vec<&String> = listing[0]
        .as_object()
        .expect("a template is an object")
        .keys()
        .collect();
    members.sort();
    s.note("template members", &json!(members));
    let welcome = listing
        .iter()
        .find(|board| board["properties"]["onboarding"] == json!(true))
        .map(id_of)
        .expect("the welcome template exists");
    let welcome_blocks = s
        .get(
            "welcome blocks",
            &format!("{ROOT_API}/boards/{welcome}/blocks"),
        )
        .user(&alice)
        .unrecorded()
        .await;
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for item in welcome_blocks
        .json()
        .as_array()
        .expect("the blocks are a list")
    {
        *counts.entry(string_member(item, "type")).or_default() += 1;
    }
    s.note("welcome template block counts", &json!(counts));
    s.get("templates: anonymous", &format!("{ROOT_API}/templates"))
        .await;
    let custom = new_board(
        s,
        &alice,
        "My template",
        "private",
        json!({"isTemplate": true}),
    )
    .await;
    s.get("templates include mine", &format!("{ROOT_API}/templates"))
        .user(&alice)
        .unrecorded()
        .await;
    s.get("my template", &format!("{ROOT_API}/containers/{custom}"))
        .user(&alice)
        .await;
    let onboard = s
        .post("onboard", &format!("{ROOT_API}/onboard"))
        .user(&alice)
        .await;
    let welcome_board = onboard.string("boardId");
    s.get(
        "welcome board",
        &format!("{ROOT_API}/containers/{welcome_board}"),
    )
    .user(&alice)
    .await;
    let copied = s
        .get(
            "welcome board blocks",
            &format!("{ROOT_API}/boards/{welcome_board}/blocks"),
        )
        .user(&alice)
        .unrecorded()
        .await;
    let copied_blocks = copied.json().as_array().map_or(0, Vec::len);
    s.note("welcome board block count", &json!(copied_blocks));
    s.post("onboard again", &format!("{ROOT_API}/onboard"))
        .user(&alice)
        .unrecorded()
        .await;
    s.get(
        "categories after the welcome board",
        &format!("{ROOT_API}/categories"),
    )
    .user(&alice)
    .await;
    s.get(
        "client state after the welcome board",
        &format!("{API}/users/me/client-state"),
    )
    .user(&alice)
    .await;
}

pub(crate) async fn files(s: &mut Session) {
    const DOWNLOAD_HEADERS: &[&str] = &[
        "Content-Type",
        "Content-Disposition",
        "Cache-Control",
        "X-Content-Type-Options",
        "Content-Security-Policy",
        "Accept-Ranges",
        "Content-Length",
    ];

    let [alice, bob] = s.users("files", ["alice", "bob"]).await;
    let board = new_board(s, &alice, "Board with files", "private", json!({})).await;
    let upload = format!("{ROOT_API}/containers/{board}/files");
    let png = || multipart("file", "pixel.png", &PNG, "image/png");

    let uploaded = s.post("upload", &upload).payload(png()).user(&alice).await;
    let file_id = uploaded.string("fileId");
    s.note(
        "uploaded name has the extension",
        &json!(has_extension(&file_id, "png")),
    );
    s.post("upload: jpeg extension", &upload)
        .payload(multipart("file", "photo.JPEG", &PNG, "image/jpeg"))
        .user(&alice)
        .await;
    let text_id = s
        .post("upload: text", &upload)
        .payload(multipart("file", "notes.txt", b"hello", "text/plain"))
        .user(&alice)
        .await
        .string("fileId");
    s.post("upload: wrong field", &upload)
        .payload(multipart("other", "pixel.png", &PNG, "image/png"))
        .user(&alice)
        .await;
    s.post("upload: non-member", &upload)
        .payload(png())
        .user(&bob)
        .await;
    s.post("upload: anonymous", &upload).payload(png()).await;
    s.post(
        "upload: unknown board",
        &format!("{ROOT_API}/containers/unknown-board/files"),
    )
    .payload(png())
    .user(&alice)
    .await;
    s.post("upload: not multipart", &upload)
        .json(json!({"file": "x"}))
        .user(&alice)
        .await;

    let files = format!("{ROOT_API}/containers/{board}/files");
    let download = format!("{files}/{file_id}");
    // The paths that named the workspace with the board are gone.
    let old_files = format!("{API}/files/teams/{ROOT_WORKSPACE_ID}/{board}/{file_id}");
    s.post(
        "old path: upload",
        &format!("{API}/teams/{ROOT_WORKSPACE_ID}/{board}/files"),
    )
    .payload(png())
    .user(&alice)
    .await;
    s.get("old path: download", &old_files).user(&alice).await;
    s.get("old path: info", &format!("{old_files}/info"))
        .user(&alice)
        .await;
    s.get("download", &download)
        .user(&alice)
        .keep_headers(DOWNLOAD_HEADERS)
        .await;
    s.get("download: text", &format!("{files}/{text_id}"))
        .user(&alice)
        .keep_headers(DOWNLOAD_HEADERS)
        .await;
    s.get("download: range", &download)
        .user(&alice)
        .header("Range", "bytes=0-7")
        .keep_headers(&["Content-Range", "Content-Length"])
        .await;
    s.get("download: range past the end", &download)
        .user(&alice)
        .header("Range", "bytes=5000-")
        .keep_headers(&["Content-Range"])
        .await;
    s.get("download: non-member", &download).user(&bob).await;
    s.get("download: anonymous", &download).await;
    s.get(
        "download: unknown file",
        &format!("{files}/7unknownfile.png"),
    )
    .user(&alice)
    .await;
    s.get(
        "download: traversal",
        &format!("{files}/..%2F..%2Fconfig.json"),
    )
    .user(&alice)
    .await;
    s.get("info", &format!("{download}/info"))
        .user(&alice)
        .await;
    s.get("info: non-member", &format!("{download}/info"))
        .user(&bob)
        .await;
    s.get(
        "info: unknown file",
        &format!("{files}/7unknownfile.png/info"),
    )
    .user(&alice)
    .await;

    // A shared board serves its files to holders of the read token.
    let read_token = s
        .post(
            "share",
            &format!("{ROOT_API}/containers/{board}/public-link"),
        )
        .json(json!({"enabled": true}))
        .user(&alice)
        .await
        .string("token");
    s.get(
        "download with the read token",
        &format!("{API}/shared/{board}/files/{file_id}?readToken={read_token}"),
    )
    .keep_headers(&["Content-Type"])
    .await;
    s.get(
        "download with a wrong read token",
        &format!("{API}/shared/{board}/files/{file_id}?readToken=wrong"),
    )
    .await;

    // An attachment block that points at the file, then deleting it.
    let card1 = new_card(
        s,
        &alice,
        &board,
        "Card with image",
        json!({}),
        "create card for attachment",
    )
    .await;
    let created = new_blocks(
        s,
        &alice,
        &board,
        json!([block(
            "img1",
            &board,
            "attachment",
            "",
            &card1,
            json!({"fileId": file_id})
        )]),
        "insert an attachment block",
    )
    .await;
    let image = created
        .as_array()
        .expect("the created blocks are a list")
        .iter()
        .find(|block| block["type"] == "attachment")
        .map(id_of)
        .expect("the attachment block was created");
    s.delete(
        "delete the image block",
        &format!("{ROOT_API}/boards/{board}/blocks/{image}"),
    )
    .user(&alice)
    .await;
    s.get(
        "blocks after deleting the image",
        &format!("{ROOT_API}/boards/{board}/blocks"),
    )
    .user(&alice)
    .await;
    s.post(
        "duplicate the board with files",
        &format!("{ROOT_API}/containers/{board}/duplicate"),
    )
    .user(&alice)
    .unrecorded()
    .await;
}

/// What an archive holds: its entry names, its version entry, and every line of its
/// `.jsonl` entries.
fn archive_summary(raw: &[u8]) -> Value {
    let mut bundle = zip::ZipArchive::new(Cursor::new(raw)).expect("the export is a ZIP archive");
    let mut names = Vec::new();
    let mut lines = Vec::new();
    let mut version = Value::Null;
    for index in 0..bundle.len() {
        let mut entry = bundle.by_index(index).expect("an archive entry opens");
        let name = entry
            .name()
            .expect("an archive entry has a readable name")
            .into_owned();
        let mut content = String::new();
        if has_extension(&name, "jsonl") || name == "version.json" {
            entry
                .read_to_string(&mut content)
                .expect("a JSON entry is text");
        }
        if has_extension(&name, "jsonl") {
            for line in content.lines().filter(|line| !line.trim().is_empty()) {
                lines.push(serde_json::from_str::<Value>(line).expect("an archive line is JSON"));
            }
        } else if name == "version.json" {
            version = serde_json::from_str(&content).expect("the version entry is JSON");
        }
        names.push(name);
    }
    // Stored files have random names. They come before the files with fixed names, in
    // the order of their extensions, so that the order is the same in every run.
    names.sort_by_cached_key(|name: &String| {
        let (directory, file) = name.rsplit_once('/').unwrap_or(("", name));
        let (stem, extension) = file.split_once('.').unwrap_or((file, ""));
        let is_fixed_name = stem.len() != fvoci_server::auth::secret::SECRET_LENGTH;
        (
            // The entries of the archive itself come after those of its boards.
            directory.is_empty(),
            directory.to_owned(),
            is_fixed_name,
            if is_fixed_name { file } else { extension }.to_owned(),
        )
    });
    json!({"names": names, "version": version, "lines": lines})
}

pub(crate) async fn archive(s: &mut Session) {
    let [alice, bob] = s.users("arch", ["alice", "bob"]).await;
    let board = new_board(
        s,
        &alice,
        "Exported board",
        "private",
        json!({
            "description": "to export",
            "cardProperties": [{"id": "p1", "name": "Status", "type": "select", "options": []}],
        }),
    )
    .await;
    let file_id = s
        .post("upload", &format!("{ROOT_API}/containers/{board}/files"))
        .payload(multipart("file", "pixel.png", &PNG, "image/png"))
        .user(&alice)
        .unrecorded()
        .await
        .string("fileId");
    let card1 = new_card(
        s,
        &alice,
        &board,
        "Exported card",
        json!({"icon": "x"}),
        "create exported card",
    )
    .await;
    new_blocks(
        s,
        &alice,
        &board,
        json!([
            block(
                "text1",
                &board,
                "comment",
                "Exported text",
                &card1,
                json!({})
            ),
            block(
                "img1",
                &board,
                "attachment",
                "",
                &card1,
                json!({"fileId": file_id})
            ),
            block(
                "view1",
                &board,
                "view",
                "Exported view",
                "",
                json!({"viewType": "table"})
            ),
        ]),
        "insert content",
    )
    .await;

    let export = format!("{ROOT_API}/containers/{board}/archive/export");
    let exported = s.get("export", &export).user(&alice).unrecorded().await;
    s.note(
        "export status and type",
        &json!([exported.status, exported.header("Content-Type")]),
    );
    let exported = exported.bytes().to_vec();
    s.note("export content", &archive_summary(&exported));
    s.get("export: non-member", &export).user(&bob).await;
    s.get("export: anonymous", &export).await;
    s.get(
        "export: unknown board",
        &format!("{ROOT_API}/containers/unknown-board/archive/export"),
    )
    .user(&alice)
    .await;
    let workspace_export = s
        .get("workspace export", &format!("{ROOT_API}/archive/export"))
        .user(&alice)
        .unrecorded()
        .await;
    s.note(
        "workspace export status and type",
        &json!([
            workspace_export.status,
            workspace_export.header("Content-Type")
        ]),
    );
    if workspace_export.status == 200 {
        let entries = archive_summary(workspace_export.bytes())["names"]
            .as_array()
            .map_or(0, Vec::len);
        s.note("workspace export entries", &json!(entries));
    }

    let import = format!("{ROOT_API}/archive/import");
    let upload = upload_archive(s, &bob, &exported).await;
    s.post("import", &import)
        .json(json!({"uploadId": upload}))
        .user(&bob)
        .await;
    s.post("import: anonymous", &format!("{ROOT_API}/archive/uploads"))
        .json(json!({"name": "export.fvoci", "size": exported.len()}))
        .await;
    let upload = upload_archive(s, &bob, b"this is not an archive").await;
    s.post("import: not an archive", &import)
        .json(json!({"uploadId": upload}))
        .user(&bob)
        .await;
    s.post("import: no such upload", &import)
        .json(json!({"uploadId": "no-such-upload"}))
        .user(&bob)
        .await;
    let imported = s
        .get("boards after import", &format!("{ROOT_API}/boards"))
        .user(&bob)
        .await;
    let copy = imported
        .json()
        .as_array()
        .expect("the boards are a list")
        .iter()
        .find(|board| board["title"] == "Exported board")
        .map(id_of)
        .expect("the imported board is listed");
    s.get(
        "imported blocks",
        &format!("{ROOT_API}/boards/{copy}/blocks"),
    )
    .user(&bob)
    .await;
    s.get(
        "imported members",
        &format!("{ROOT_API}/containers/{copy}/members"),
    )
    .user(&bob)
    .await;
    let images = s
        .get(
            "imported attachment block",
            &format!("{ROOT_API}/boards/{copy}/blocks?type=attachment"),
        )
        .user(&bob)
        .unrecorded()
        .await;
    let imported_file = string_member(&images.json()[0]["fields"], "fileId");
    s.get(
        "imported file",
        &format!("{ROOT_API}/containers/{copy}/files/{imported_file}"),
    )
    .user(&bob)
    .await;

    // Retired line-based version 1 archive format, which is refused because it is not a ZIP.
    let legacy_lines = [
        json!({"version": 1, "date": 1}),
        json!({"type": "block", "data": {
            "id": "legacy-board", "type": "board", "title": "Legacy board",
            "fields": {"icon": "x", "cardProperties": []}, "createdAt": 1, "updatedAt": 1,
            "parentId": "", "rootId": "legacy-board",
        }}),
        json!({"type": "block", "data": {
            "id": "legacy-card", "type": "card", "title": "Legacy card",
            "fields": {}, "createdAt": 1, "updatedAt": 1,
            "parentId": "legacy-board", "rootId": "legacy-board",
        }}),
    ];
    let legacy = legacy_lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let upload = upload_archive(s, &bob, legacy.as_bytes()).await;
    s.post("import: legacy format", &import)
        .json(json!({"uploadId": upload}))
        .user(&bob)
        .await;
    let boards = s
        .get("boards after legacy import", &format!("{ROOT_API}/boards"))
        .user(&bob)
        .unrecorded()
        .await;
    let boards = boards.json();
    let titles: Vec<&str> = boards
        .as_array()
        .expect("the boards are a list")
        .iter()
        .filter_map(|board| board["title"].as_str())
        .collect();
    s.note("board titles after legacy import", &json!(titles));
}

/// Opens an upload of an archive into the root workspace as `user`, sends its parts, and
/// returns its identifier. Nothing of it is recorded: the upload's identifier and end
/// differ from run to run.
async fn upload_archive(s: &mut Session, user: &str, bytes: &[u8]) -> String {
    let opened = s
        .post(
            "open an upload of an archive",
            &format!("{ROOT_API}/archive/uploads"),
        )
        .json(json!({"name": "export.fvoci", "size": bytes.len()}))
        .user(user)
        .unrecorded()
        .await;
    let upload = string_member(opened.json(), "uploadId");
    let part_size = opened.json()["partSize"]
        .as_u64()
        .and_then(|size| usize::try_from(size).ok())
        .expect("an upload has a part size");
    for (index, part) in bytes.chunks(part_size).enumerate() {
        let digest = format!("sha-256=:{}:", STANDARD.encode(Sha256::digest(part)));
        let sent = s
            .put(
                "send a part of an archive",
                &format!("{ROOT_API}/uploads/{upload}/parts/{}", index + 1),
            )
            .payload(Payload::Raw {
                bytes: part.to_vec(),
                content_type: Some(OCTET_STREAM.to_owned()),
            })
            .header("Content-Digest", &digest)
            .user(user)
            .unrecorded()
            .await;
        assert_eq!(sent.status, 204, "the part is received");
    }
    upload
}
