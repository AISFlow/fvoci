//! Archive export and import.
use std::io::{Cursor, Read as _, Write as _};

use crate::common::ROOT_WS;
use crate::common::TEST_WS;
use crate::common::{ApiResponse, Client, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TestServer};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fvoci_model::{Block, BlockType, Container, ContainerVisibility, new_id};
use reqwest::{Method, StatusCode};
use sha2::{Digest as _, Sha256};

/// `GET /containers/{id}/archive/export` for a board.
async fn export_board(client: &Client, board: &Container) -> ApiResponse {
    client
        .get(&format!(
            "/w/{}/containers/{}/archive/export",
            board.workspace_id, board.id
        ))
        .await
}

/// Imports archive bytes into a workspace: opens an upload of the archive, sends its parts
/// and imports it. The first answer that is not a success is returned, else the
/// import's.
pub(crate) async fn import(client: &Client, workspace_id: &str, bytes: &[u8]) -> ApiResponse {
    let opened = client
        .post(
            &format!("/w/{workspace_id}/archive/uploads"),
            &serde_json::json!({"name": "archive.fvoci", "size": bytes.len()}),
        )
        .await;
    if opened.status != StatusCode::CREATED {
        return opened;
    }
    let upload_id = opened.value()["uploadId"]
        .as_str()
        .expect("an upload has an identifier")
        .to_owned();
    let part_size = opened.value()["partSize"]
        .as_u64()
        .and_then(|size| usize::try_from(size).ok())
        .expect("an upload has a part size");
    for (index, part) in bytes.chunks(part_size).enumerate() {
        let sent = put_part(client, workspace_id, &upload_id, index + 1, part).await;
        if sent.status != StatusCode::NO_CONTENT {
            return sent;
        }
    }
    import_upload(client, workspace_id, &upload_id).await
}

/// `importArchive` of an upload.
pub(crate) async fn import_upload(
    client: &Client,
    workspace_id: &str,
    upload_id: &str,
) -> ApiResponse {
    client
        .post(
            &format!("/w/{workspace_id}/archive/import"),
            &serde_json::json!({"uploadId": upload_id}),
        )
        .await
}

/// Sends `bytes` as part `number` of an upload, with their digest.
pub(crate) async fn put_part(
    client: &Client,
    workspace_id: &str,
    upload_id: &str,
    number: usize,
    bytes: &[u8],
) -> ApiResponse {
    let digest = format!("sha-256=:{}:", STANDARD.encode(Sha256::digest(bytes)));
    let request = client
        .request(
            Method::PUT,
            &format!("/w/{workspace_id}/uploads/{upload_id}/parts/{number}"),
        )
        .header("Content-Type", "application/octet-stream")
        .header("Content-Digest", digest)
        .body(bytes.to_vec());
    Client::send(request).await
}

/// Makes a board with one card for the first client.
async fn board_with_card(ts: &TestServer, workspace_id: &str) -> Container {
    let board = ts
        .create_board(workspace_id, ContainerVisibility::Open)
        .await;
    let card = Block {
        id: new_id(),
        parent_id: String::new(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        title: "a card for export".to_owned(),
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    crate::blocks::insert_in(
        &ts.client,
        workspace_id,
        &board.id,
        std::slice::from_ref(&card),
    )
    .await;
    board
}

/// Exporting a single board and re-importing it preserves its card.
#[tokio::test]
async fn export_single_board_round_trips() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = board_with_card(&ts, TEST_WORKSPACE_ID).await;

    let exported = export_board(&ts.client, &board).await;
    exported.check_ok();
    assert!(!exported.body.is_empty(), "the archive has bytes");

    import(&ts.client, ROOT_WORKSPACE_ID, &exported.body)
        .await
        .check_ok();

    let user_id = ts.client.me().await.id;
    let boards = ts
        .app
        .containers_for_user_and_workspace(
            &user_id,
            ROOT_WORKSPACE_ID,
            fvoci_model::PublicContainers::Include,
        )
        .await
        .expect("the imported boards");
    assert!(!boards.is_empty(), "a board was imported into workspace 0");
    let board_id = boards[0].id.clone();
    let blocks = ts
        .app
        .blocks_for_container(&board_id)
        .await
        .expect("the imported blocks");
    assert!(
        blocks
            .iter()
            .any(|block| block.title == "a card for export"),
        "the imported board keeps its card"
    );
    ts.teardown().await;
}

/// A board without blocks exports and imports cleanly as a board without blocks.
#[tokio::test]
async fn export_of_a_board_without_blocks_round_trips() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    let exported = export_board(&ts.client, &board).await;
    exported.check_ok();
    import(&ts.client, ROOT_WORKSPACE_ID, &exported.body)
        .await
        .check_ok();

    let user_id = ts.client.me().await.id;
    let boards = ts
        .app
        .containers_for_user_and_workspace(
            &user_id,
            ROOT_WORKSPACE_ID,
            fvoci_model::PublicContainers::Include,
        )
        .await
        .expect("the imported boards");
    assert_eq!(boards.len(), 1, "the board was imported into workspace 0");
    let blocks = ts
        .app
        .blocks_for_container(&boards[0].id)
        .await
        .expect("the imported blocks");
    assert!(blocks.is_empty(), "without blocks");
    ts.teardown().await;
}

/// An archive export carries the expected binary download and attachment headers.
#[tokio::test]
async fn export_has_the_download_headers() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = board_with_card(&ts, TEST_WORKSPACE_ID).await;
    let response = export_board(&ts.client, &board).await;
    response.check_ok();
    assert_eq!(
        response.header("content-type"),
        "application/octet-stream",
        "content type"
    );
    assert!(
        response
            .header("content-disposition")
            .starts_with("attachment; filename=archive-"),
        "disposition: {}",
        response.header("content-disposition")
    );
    assert!(
        std::path::Path::new(&response.header("content-disposition"))
            .extension()
            .is_some_and(|ext| ext == "fvoci"),
        "the extension"
    );
    assert_eq!(
        response.header("content-transfer-encoding"),
        "binary",
        "transfer encoding"
    );

    // The body is a ZIP with version 1 and the container's directory.
    let mut zip = zip::ZipArchive::new(Cursor::new(response.body.clone())).expect("a zip");
    let mut version = String::new();
    zip.by_name("version.json")
        .expect("version.json")
        .read_to_string(&mut version)
        .expect("read");
    assert!(version.contains("\"version\":1"), "version 1: {version}");
    assert!(
        (0..zip.len()).any(|index| {
            zip.by_index(index)
                .expect("entry")
                .name()
                .is_ok_and(|name| name.ends_with("container.jsonl"))
        }),
        "a container.jsonl is present"
    );
    ts.teardown().await;
}

/// The lines of `container.jsonl` keep the contract's field name: a block and a member
/// name their container `containerId`, never `boardId`.
#[tokio::test]
async fn an_exported_block_and_member_name_their_board_by_the_archives_name() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = board_with_card(&ts, TEST_WORKSPACE_ID).await;
    let response = export_board(&ts.client, &board).await;
    response.check_ok();
    let mut zip = zip::ZipArchive::new(Cursor::new(response.body.clone())).expect("a zip");
    let jsonl = zip
        .file_names()
        .filter_map(Result::ok)
        .find(|name| name.ends_with("container.jsonl"))
        .map(std::borrow::Cow::into_owned)
        .expect("a container.jsonl is present");
    let mut lines = String::new();
    zip.by_name(&jsonl)
        .expect("container.jsonl")
        .read_to_string(&mut lines)
        .expect("read");
    let mut kinds = Vec::new();
    for line in lines.lines() {
        let line: serde_json::Value = serde_json::from_str(line).expect("a JSON line");
        let kind = line["type"].as_str().expect("a line type").to_owned();
        if kind == "block" || kind == "containerMember" {
            assert_eq!(line["data"]["containerId"], board.id.as_str(), "{line}");
            assert!(line["data"].get("boardId").is_none(), "{line}");
        }
        kinds.push(kind);
    }
    for kind in ["container", "block", "containerMember"] {
        assert!(
            kinds.iter().any(|one| one == kind),
            "a {kind} line: {kinds:?}"
        );
    }
    ts.teardown().await;
}

/// Export is allowed for members and refused for non-members and anonymous clients.
#[tokio::test]
async fn export_container_access_control() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = board_with_card(&ts, TEST_WORKSPACE_ID).await;

    export_board(&ts.client, &board).await.check_ok();
    export_board(&ts.client2, &board).await.check_forbidden();
    export_board(&ts.anonymous(), &board)
        .await
        .check_unauthorized();
    ts.teardown().await;
}

/// Workspace archive export returns a 200 ZIP for members and refuses anonymous callers.
#[tokio::test]
async fn export_workspace_is_a_zip() {
    let ts = Setup::native().start().await.init_basic().await;
    board_with_card(&ts, ROOT_WORKSPACE_ID).await;
    let response = ts.client.get(&format!("{ROOT_WS}/archive/export")).await;
    response.check_ok();
    assert!(
        zip::ZipArchive::new(Cursor::new(response.body.clone())).is_ok(),
        "a zip"
    );
    ts.anonymous()
        .get(&format!("{ROOT_WS}/archive/export"))
        .await
        .check_unauthorized();
    ts.teardown().await;
}

/// Any signed-in user may import; an anonymous one may not.
#[tokio::test]
async fn import_is_allowed_for_any_signed_in_user() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = board_with_card(&ts, ROOT_WORKSPACE_ID).await;
    let archive = export_board(&ts.client, &board).await.body;

    import(&ts.client2, ROOT_WORKSPACE_ID, &archive)
        .await
        .check_ok();
    import(&ts.anonymous(), ROOT_WORKSPACE_ID, &archive)
        .await
        .check_unauthorized();
    ts.teardown().await;
}

/// Importing non-archive bytes is rejected as a 400 bad request.
#[tokio::test]
async fn import_of_a_non_archive_is_a_bad_request() {
    let ts = Setup::native().start().await.init_basic().await;
    let response = import(&ts.client, ROOT_WORKSPACE_ID, b"this is not a zip file").await;
    response.check_bad_request();
    assert_eq!(
        response.problem_type(),
        "/problems/bad-request",
        "{}",
        response.text()
    );
    ts.teardown().await;
}

/// An empty ZIP is no archive either.
#[tokio::test]
async fn import_of_an_empty_zip_is_a_bad_request() {
    let ts = Setup::native().start().await.init_basic().await;
    let empty = zip::ZipWriter::new(Cursor::new(Vec::new()))
        .finish()
        .expect("an empty zip")
        .into_inner();
    let response = import(&ts.client, ROOT_WORKSPACE_ID, &empty).await;
    response.check_bad_request();
    assert_eq!(
        response.problem_type(),
        "/problems/bad-request",
        "{}",
        response.text()
    );
    ts.teardown().await;
}

/// Nor is an archive of a version this server does not read.
#[tokio::test]
async fn import_of_an_unsupported_version_is_a_bad_request() {
    let ts = Setup::native().start().await.init_basic().await;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("version.json", zip::write::SimpleFileOptions::default())
        .expect("start");
    writer
        .write_all(br#"{"version":99,"date":1}"#)
        .expect("write");
    let bytes = writer.finish().expect("finish").into_inner();
    let response = import(&ts.client, ROOT_WORKSPACE_ID, &bytes).await;
    response.check_bad_request();
    assert_eq!(
        response.problem_type(),
        "/problems/bad-request",
        "{}",
        response.text()
    );
    ts.teardown().await;
}

/// A hostile ZIP whose entry name escapes its directory is imported harmlessly: the file
/// name is reduced to its base, so nothing is written outside the uploads root, and the
/// orphan image (no board) is skipped.
#[tokio::test]
async fn import_of_a_traversing_entry_is_harmless() {
    let ts = Setup::native().start().await.init_basic().await;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("version.json", options).expect("start");
    writer
        .write_all(br#"{"version":1,"date":1}"#)
        .expect("write");
    writer
        .start_file("board1/../../../evil.txt", options)
        .expect("start file");
    writer.write_all(b"pwned").expect("write");
    let bytes = writer.finish().expect("finish").into_inner();

    // No board directory matches the orphan file, so it is skipped and the import
    // succeeds.
    import(&ts.client, ROOT_WORKSPACE_ID, &bytes)
        .await
        .check_ok();
    ts.teardown().await;
}

/// Two blocks that name the same file: it is written once, as an entry has a name of its
/// own. Found when an export of such a board failed. A third block's own file is
/// exported too.
#[tokio::test]
async fn export_of_a_board_whose_blocks_share_a_file() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = board_with_card(&ts, TEST_WORKSPACE_ID).await;
    let cards: Vec<Block> = ts
        .client
        .get(&format!("{TEST_WS}/boards/{}/blocks?type=card", board.id))
        .await
        .ok_json();
    let card_id = cards[0].id.clone();
    let upload = async |name: &str| {
        crate::files::uploaded_id(&crate::files::upload(&ts.client, &board.id, name, b"x").await)
    };
    let shared = upload("shared.png").await;
    let own = upload("own.png").await;
    let attachment = |file: &str| {
        let mut fields = fvoci_model::JsonMap::new();
        fields.insert(
            "fileId".to_owned(),
            serde_json::Value::String(file.to_owned()),
        );
        fvoci_model::Block {
            id: fvoci_model::new_id(),
            container_id: board.id.clone(),
            parent_id: card_id.clone(),
            block_type: fvoci_model::BlockType::Attachment,
            fields,
            created_at: 1,
            updated_at: 1,
            ..fvoci_model::Block::default()
        }
    };
    ts.client
        .insert_blocks(&board.id, &[attachment(&shared)])
        .await
        .check_created();
    // Another block shares the file, and a third has one that only it shows.
    let mut others = [attachment(&shared), attachment(&own)];
    ts.app
        .insert_blocks(
            &mut others,
            &ts.stranger().await,
            fvoci_server::app::ChangeNotification::Send,
        )
        .await
        .expect("the blocks are stored");

    let export = export_board(&ts.client, &board).await;
    export.check_ok();
    let archive = zip::ZipArchive::new(std::io::Cursor::new(export.body.clone()))
        .expect("the export is an archive");
    let mut files: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .filter(|name| name.ends_with(".png"))
        .map(|name| name.rsplit('/').next().unwrap_or_default().to_owned())
        .collect();
    files.sort();
    let mut expected = [shared, own];
    expected.sort();
    assert_eq!(
        files, expected,
        "the shared file has one entry, and the third block's own file one too"
    );
    ts.teardown().await;
}

/// The boards of a workspace that the first client sees.
pub(crate) async fn boards_of(ts: &TestServer) -> Vec<Container> {
    ts.client.get(&format!("{TEST_WS}/boards")).await.ok_json()
}

/// Non-v1 archive versions (ZIP version 2 or 3, line-based non-ZIP) and a ZIP without
/// `version.json` are refused, and nothing of them is stored.
#[tokio::test]
async fn archives_of_the_former_formats_are_refused() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = serde_json::json!({"type": "container", "data": {
        "id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Former", "createdAt": 1, "updatedAt": 1,
    }});
    let zip_of = |entries: &[(&str, String)]| {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .expect("start");
            writer.write_all(content.as_bytes()).expect("write");
        }
        writer.finish().expect("finish").into_inner()
    };
    let version_two = zip_of(&[
        ("version.json", r#"{"version":2,"date":1}"#.to_owned()),
        ("b1/container.jsonl", format!("{board}\n")),
    ]);
    let version_three = zip_of(&[
        ("version.json", r#"{"version":3,"date":1}"#.to_owned()),
        ("b1/container.jsonl", format!("{board}\n")),
    ]);
    let without_version = zip_of(&[("b1/container.jsonl", format!("{board}\n"))]);
    let legacy = format!(
        "{{\"version\":1,\"date\":1}}\n{}\n",
        serde_json::json!({"type": "block", "data": {
            "id": "b1", "type": "board", "title": "Former", "createdAt": 1, "updatedAt": 1,
        }})
    );
    let legacy_board_line = zip_of(&[
        ("version.json", r#"{"version":1,"date":1}"#.to_owned()),
        (
            "b1/container.jsonl",
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "board",
                    "data": {"id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Former"}
                })
            ),
        ),
    ]);
    let legacy_flag = concat!("scheme", "Admin");
    let legacy_member_line = zip_of(&[
        ("version.json", r#"{"version":1,"date":1}"#.to_owned()),
        (
            "b1/container.jsonl",
            format!(
                "{board}\n{}\n",
                serde_json::json!({
                    "type": "boardMember",
                    "data": {"containerId": "b1", "userId": "u1", (legacy_flag): true}
                })
            ),
        ),
    ]);
    let legacy_container_member_flags = zip_of(&[
        ("version.json", r#"{"version":1,"date":1}"#.to_owned()),
        (
            "b1/container.jsonl",
            format!(
                "{board}\n{}\n",
                serde_json::json!({
                    "type": "containerMember",
                    "data": {"containerId": "b1", "userId": "u1", (legacy_flag): true}
                })
            ),
        ),
    ]);
    let before = boards_of(&ts).await.len();
    for (what, bytes) in [
        ("version 2", version_two),
        ("version 3", version_three),
        ("no version.json", without_version),
        ("non-zip jsonl", legacy.into_bytes()),
        ("legacy board line", legacy_board_line),
        ("legacy boardMember line", legacy_member_line),
        (
            "legacy containerMember flags",
            legacy_container_member_flags,
        ),
    ] {
        let response = import(&ts.client, TEST_WORKSPACE_ID, &bytes).await;
        response.check_bad_request();
        assert_eq!(
            response.problem_type(),
            "/problems/bad-request",
            "{what}: {}",
            response.text()
        );
    }
    assert_eq!(boards_of(&ts).await.len(), before, "no board was made");
    ts.teardown().await;
}

/// Version-1 `.fvoci` archives whose container line carries retired option shapes (`value`
/// instead of `label`, or the retired color prefix) are refused as 400 bad requests rather
/// than normalized through a legacy compatibility path.
#[tokio::test]
async fn archive_with_retired_card_property_option_shapes_is_refused() {
    let ts = Setup::native().start().await.init_basic().await;
    let zip_of = |container_line: serde_json::Value| {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        writer.start_file("version.json", opts).expect("start");
        writer
            .write_all(br#"{"version":1,"date":1}"#)
            .expect("write");
        writer
            .start_file("b1/container.jsonl", opts)
            .expect("start");
        writer
            .write_all(format!("{container_line}\n").as_bytes())
            .expect("write");
        writer.finish().expect("finish").into_inner()
    };
    let retired_value_key = zip_of(serde_json::json!({
        "type": "container",
        "data": {
            "id": "b1",
            "workspaceId": TEST_WORKSPACE_ID,
            "visibility": "private",
            "title": "Retired Option Key",
            "cardProperties": [{
                "id": "status",
                "name": "Status",
                "type": "select",
                "options": [{"id": "o1", "value": "Open", "color": "red"}],
            }],
            "createdAt": 1,
            "updatedAt": 1,
        }
    }));
    let retired_color_prefix = zip_of(serde_json::json!({
        "type": "container",
        "data": {
            "id": "b1",
            "workspaceId": TEST_WORKSPACE_ID,
            "visibility": "private",
            "title": "Retired Option Color",
            "cardProperties": [{
                "id": "status",
                "name": "Status",
                "type": "select",
                "options": [{"id": "o1", "label": "Open", "color": concat!("prop", "ColorRed")}],
            }],
            "createdAt": 1,
            "updatedAt": 1,
        }
    }));
    let before = boards_of(&ts).await.len();
    for (what, bytes) in [
        ("retired option value key", retired_value_key),
        ("retired option color prefix", retired_color_prefix),
    ] {
        let response = import(&ts.client, TEST_WORKSPACE_ID, &bytes).await;
        response.check_bad_request();
        assert_eq!(
            response.problem_type(),
            "/problems/bad-request",
            "{what}: {}",
            response.text()
        );
    }
    assert_eq!(boards_of(&ts).await.len(), before, "no board was made");
    ts.teardown().await;
}

/// An archive carries the files that the images of bodies show, and an import points
/// the bodies at the board's new files, under the names they were uploaded with. An
/// image block without a file, which an export used to answer with 500, is exported as
/// it is.
#[tokio::test]
async fn an_archive_carries_the_images_of_bodies() {
    an_archive_carries_the_files_of_bodies("image").await;
}

#[tokio::test]
async fn an_archive_carries_the_files_of_file_blocks() {
    an_archive_carries_the_files_of_bodies("file").await;
}

/// The first line of the board's `container.jsonl` in an archive: the board's own.
fn board_line(mut archive: zip::ZipArchive<Cursor<Vec<u8>>>, board_id: &str) -> serde_json::Value {
    let mut lines = String::new();
    std::io::Read::read_to_string(
        &mut archive
            .by_name(&format!("{board_id}/container.jsonl"))
            .expect("the board's lines"),
        &mut lines,
    )
    .expect("text");
    serde_json::from_str(lines.lines().next().expect("a first line")).expect("JSON")
}

async fn an_archive_carries_the_files_of_bodies(kind: &str) {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let file_id = crate::files::uploaded_id(
        &crate::files::upload(&ts.client, &board.id, "picture.png", b"png").await,
    );
    let body = serde_json::json!({"type": "doc", "content": [
        {"type": kind, "attrs": {"fileId": file_id}},
    ]});
    let card: serde_json::Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &serde_json::json!({"title": "Card", "body": body}),
        )
        .await
        .created_json();
    let without_file = Block {
        id: new_id(),
        parent_id: card["id"].as_str().expect("card id").to_owned(),
        container_id: board.id.clone(),
        block_type: BlockType::Attachment,
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    ts.client
        .insert_blocks(&board.id, std::slice::from_ref(&without_file))
        .await
        .check_created();

    let exported = export_board(&ts.client, &board).await;
    exported.check_ok();
    let archive = zip::ZipArchive::new(Cursor::new(exported.body.clone())).expect("a zip");
    let entry = format!("{}/{file_id}", board.id);
    assert!(
        archive
            .file_names()
            .any(|name| name.is_ok_and(|name| name == entry.as_str())),
        "the body's image is in the archive: {:?}",
        archive.file_names().collect::<Vec<_>>()
    );
    // The board's line records the original upload file name under `fileNames`.
    assert_eq!(
        board_line(archive, &board.id)["data"]["fileNames"],
        serde_json::json!({ file_id.as_str(): "picture.png" }),
        "the board's line names its file"
    );

    import(&ts.client, TEST_WORKSPACE_ID, &exported.body)
        .await
        .check_ok();
    let imported = boards_of(&ts)
        .await
        .into_iter()
        .find(|other| other.id != board.id)
        .expect("the imported board");
    let blocks: Vec<serde_json::Value> = ts
        .client
        .get(&format!(
            "{TEST_WS}/boards/{}/blocks?type=card",
            imported.id
        ))
        .await
        .ok_json();
    let new_file = blocks[0]["body"]["content"][0]["attrs"]["fileId"]
        .as_str()
        .expect("the image names a file")
        .to_owned();
    assert_ne!(new_file, file_id, "a file of the new board");
    assert_eq!(
        blocks[0]["body"]["content"][0]["type"],
        serde_json::json!(kind),
        "still the same node"
    );
    let file = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{new_file}",
            imported.id
        ))
        .await;
    file.check_ok();
    assert_eq!(
        file.body.as_slice(),
        b"png",
        "with the content of the old one"
    );
    // Under the name it was uploaded with, which a file block shows, not its stored name.
    let info: serde_json::Value = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{new_file}/info",
            imported.id
        ))
        .await
        .ok_json();
    assert_eq!(info["name"], serde_json::json!("picture.png"), "{info}");
    ts.teardown().await;
}

/// An imported comment whose body shows an image is pointed at the new board's file
/// without being edited: it says no `editedAt` and has no versions, as the archive
/// carries none.
#[tokio::test]
async fn an_imported_comment_with_an_image_is_not_edited() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let file_id = crate::files::uploaded_id(
        &crate::files::upload(&ts.client, &board.id, "picture.png", b"png").await,
    );
    let card: serde_json::Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &serde_json::json!({"title": "Card"}),
        )
        .await
        .created_json();
    let body = serde_json::json!({"type": "doc", "content": [
        {"type": "image", "attrs": {"fileId": file_id}},
    ]});
    ts.client
        .post(
            &format!("{TEST_WS}/boards/{}/blocks", board.id),
            &serde_json::json!([{"containerId": board.id, "parentId": card["id"], "type": "comment",
                                 "createdAt": 1, "updatedAt": 1, "body": body}]),
        )
        .await
        .check_created();

    let exported = export_board(&ts.client, &board).await;
    exported.check_ok();
    import(&ts.client, TEST_WORKSPACE_ID, &exported.body)
        .await
        .check_ok();
    let imported = boards_of(&ts)
        .await
        .into_iter()
        .find(|other| other.id != board.id)
        .expect("the imported board");
    let comments: Vec<serde_json::Value> = ts
        .client
        .get(&format!(
            "{TEST_WS}/boards/{}/blocks?type=comment",
            imported.id
        ))
        .await
        .ok_json();
    let [comment] = comments.as_slice() else {
        panic!("one comment: {comments:?}");
    };
    let new_file = comment["body"]["content"][0]["attrs"]["fileId"]
        .as_str()
        .expect("the image names a file");
    assert_ne!(new_file, file_id, "a file of the new board");
    assert_eq!(comment.get("editedAt"), None, "not edited: {comment}");
    let versions = ts
        .client
        .get(&format!(
            "{TEST_WS}/boards/{}/blocks/{}/versions",
            imported.id,
            comment["id"].as_str().expect("an id")
        ))
        .await
        .value();
    assert_eq!(versions, serde_json::json!([]));
    ts.client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{new_file}",
            imported.id
        ))
        .await
        .check_ok();
    ts.teardown().await;
}

/// A comment is on a card: an archive with a comment on the board itself, or on another
/// comment, is refused and stores nothing.
#[tokio::test]
async fn an_archive_with_a_comment_off_a_card_stores_nothing() {
    let ts = Setup::native().start().await.init_basic().await;
    let line = |kind: &str, data: serde_json::Value| {
        format!("{}\n", serde_json::json!({"type": kind, "data": data}))
    };
    let block = |id: &str, parent: &str, kind: &str| {
        line(
            "block",
            serde_json::json!({"id": id, "containerId": "b1", "parentId": parent, "type": kind,
                               "title": id, "createdAt": 1, "updatedAt": 1}),
        )
    };
    let board = line(
        "container",
        serde_json::json!({"id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Comments",
                           "createdAt": 1, "updatedAt": 1}),
    );
    let before = boards_of(&ts).await.len();
    for blocks in [
        block("c1", "", "comment"),
        [
            block("k1", "", "card"),
            block("c1", "k1", "comment"),
            block("c2", "c1", "comment"),
        ]
        .concat(),
    ] {
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer.start_file("version.json", stored).expect("start");
        writer
            .write_all(br#"{"version":1,"date":1}"#)
            .expect("write");
        writer
            .start_file("b1/container.jsonl", stored)
            .expect("start");
        writer
            .write_all(format!("{board}{blocks}").as_bytes())
            .expect("write");
        let bytes = writer.finish().expect("finish").into_inner();
        let response = import(&ts.client, TEST_WORKSPACE_ID, &bytes).await;
        response.check_error(
            StatusCode::BAD_REQUEST,
            "the parent of a comment is a card of the board, a page of the wiki or a unit of the course",
        );
        assert_eq!(boards_of(&ts).await.len(), before, "no board was made");
    }
    ts.teardown().await;
}

/// An archive with an entry that cannot be read (here its checksum does not match) is
/// refused before anything of it is stored.
#[tokio::test]
async fn an_archive_with_a_broken_entry_stores_nothing() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = serde_json::json!({"type": "container", "data": {
        "id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Broken", "createdAt": 1, "updatedAt": 1,
    }});
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("version.json", stored).expect("start");
    writer
        .write_all(br#"{"version":1,"date":1}"#)
        .expect("write");
    writer
        .start_file("b1/container.jsonl", stored)
        .expect("start");
    writer
        .write_all(format!("{board}\n").as_bytes())
        .expect("write");
    writer.start_file("b1/picture.png", stored).expect("start");
    writer.write_all(b"the bytes of a picture").expect("write");
    let mut bytes = writer.finish().expect("finish").into_inner();
    let at = bytes
        .windows(5)
        .position(|window| window == b"bytes")
        .expect("the stored content");
    bytes[at] = b'B';

    let before = boards_of(&ts).await.len();
    let response = import(&ts.client, TEST_WORKSPACE_ID, &bytes).await;
    response.check_bad_request();
    assert_eq!(boards_of(&ts).await.len(), before, "no board was made");
    ts.teardown().await;
}

/// An attachment block may name its file as a client wrote it. A name that cannot be a
/// stored name gets no entry, so the archive is exported and read back.
#[tokio::test]
async fn an_archive_reads_back_whatever_an_attachment_names() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = board_with_card(&ts, TEST_WORKSPACE_ID).await;
    let cards: Vec<Block> = ts
        .client
        .get(&format!("{TEST_WS}/boards/{}/blocks?type=card", board.id))
        .await
        .ok_json();
    let card_id = cards[0].id.clone();
    let image = |file: &str| {
        let mut fields = fvoci_model::JsonMap::new();
        fields.insert(
            "fileId".to_owned(),
            serde_json::Value::String(file.to_owned()),
        );
        Block {
            id: new_id(),
            container_id: board.id.clone(),
            parent_id: card_id.clone(),
            block_type: BlockType::Attachment,
            fields,
            created_at: 1,
            updated_at: 1,
            ..Block::default()
        }
    };
    ts.client
        .insert_blocks(
            &board.id,
            &[
                image("container.jsonl"),
                image("x/container.jsonl"),
                image("version.json"),
            ],
        )
        .await
        .check_created();

    let exported = export_board(&ts.client, &board).await;
    exported.check_ok();
    let before = boards_of(&ts).await.len();
    import(&ts.client, TEST_WORKSPACE_ID, &exported.body)
        .await
        .check_ok();
    assert_eq!(boards_of(&ts).await.len(), before + 1, "one board was made");
    ts.teardown().await;
}

/// A workspace's archive of two boards: the first is fine, the second has a card and the
/// given blocks below it.
pub(crate) fn two_board_archive(second_blocks: &[serde_json::Value]) -> Vec<u8> {
    let board = |id: &str| {
        serde_json::json!({"type": "container", "data": {
            "id": id, "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": id, "createdAt": 1,
            "updatedAt": 1,
        }})
    };
    let card = |id: &str, board_id: &str| {
        serde_json::json!({"type": "block", "data": {
            "id": id, "containerId": board_id, "type": "card", "title": "card", "createdAt": 1,
            "updatedAt": 1,
        }})
    };
    let first = vec![board("b1"), card("c1", "b1")];
    let mut second = vec![board("b2"), card("c2", "b2")];
    second.extend(second_blocks.iter().cloned());
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("version.json", options).expect("start");
    writer
        .write_all(br#"{"version":1,"date":1}"#)
        .expect("write");
    for (dir, lines) in [("b1", &first), ("b2", &second)] {
        writer
            .start_file(format!("{dir}/container.jsonl"), options)
            .expect("start");
        for line in lines {
            writer
                .write_all(format!("{line}\n").as_bytes())
                .expect("write");
        }
    }
    writer.finish().expect("finish").into_inner()
}

/// An archive whose second board the server refuses stores neither board: every board
/// is read and checked before the first is stored. The second board holds a block of a
/// type no new block has, as an export of a board that an older server stored can, or a
/// card whose fields are over the limit of a card, as a card stored before the limit
/// can be.
#[tokio::test]
async fn an_archive_with_a_refused_board_stores_nothing() {
    let ts = Setup::native().start().await.init_basic().await;
    let unknown_type = serde_json::json!({"type": "block", "data": {
        "id": "t2", "containerId": "b2", "parentId": "c2", "type": "text", "title": "text",
        "createdAt": 1, "updatedAt": 1,
    }});
    let non_board_type = serde_json::json!({"type": "block", "data": {
        "id": "p2", "containerId": "b2", "parentId": "", "type": "page", "title": "page",
        "createdAt": 1, "updatedAt": 1,
    }});
    let oversized = serde_json::json!({"type": "block", "data": {
        "id": "c3", "containerId": "b2", "type": "card", "title": "big", "createdAt": 1,
        "updatedAt": 1, "fields": {"note": "x".repeat(70_000)},
    }});
    for (block, says) in [
        (unknown_type, "unknown variant `text`"),
        (
            non_board_type,
            "a page is a block of a wiki, not of a board",
        ),
        (oversized, "block fields size limit exceeded"),
    ] {
        let before = boards_of(&ts).await.len();
        let response = import(&ts.client, TEST_WORKSPACE_ID, &two_board_archive(&[block])).await;
        response.check_bad_request();
        assert!(
            response.value()["detail"]
                .as_str()
                .is_some_and(|message| message.contains(says)),
            "the refusal says {says}: {}",
            response.text()
        );
        assert_eq!(boards_of(&ts).await.len(), before, "no board was made");
    }
    ts.teardown().await;
}

/// An archive is written whole to a temporary file before it is sent, so the answer
/// says its length; nothing of the file is left once it is sent.
#[tokio::test]
async fn an_export_is_sent_with_its_length_and_leaves_nothing() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = board_with_card(&ts, TEST_WORKSPACE_ID).await;

    let exported = export_board(&ts.client, &board).await;
    exported.check_ok();
    assert_eq!(
        exported.header("Content-Length"),
        exported.body.len().to_string(),
        "the length of the archive"
    );
    // Whoever has the whole archive finds nothing of it held: the place of the export
    // is free for the next one, and the file goes when the sent stream is dropped,
    // which the end of the body does not wait for.
    let exports = ts.app.config().tmp_dir().join("exports");
    until("the temporary archive is removed", || {
        temporary_archives(&ts) == 0
    })
    .await;
    let left = std::fs::read_dir(&exports).map_or(0, Iterator::count);
    assert_eq!(left, 0, "no temporary archive is left");
    export_board(&ts.client, &board).await.check_ok();
    until("the archive of the export after it is removed", || {
        temporary_archives(&ts) == 0
    })
    .await;
    let left = std::fs::read_dir(&exports).map_or(0, Iterator::count);
    assert_eq!(left, 0, "nor of the export after it");

    // What a stopped server left is removed when it starts.
    std::fs::create_dir_all(&exports).expect("the directory is made");
    std::fs::write(exports.join("stopped.fvoci"), b"half").expect("written");
    let removed = ts
        .app
        .sweep_temporary_files()
        .await
        .expect("the sweep runs");
    assert_eq!(removed, 1);
    let left = std::fs::read_dir(&exports).map_or(0, Iterator::count);
    assert_eq!(left, 0, "the left-over archive is gone");
}

/// A file larger than what the archive writer gathers at a time is written across
/// several pieces of its entry, and comes back whole.
#[tokio::test]
async fn a_large_file_is_whole_in_the_archive() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let data: Vec<u8> = (0..5 * 1024 * 1024 / 2)
        .map(|index: usize| u8::try_from(index % 251).expect("below 251"))
        .collect();
    let response = crate::files::upload(&ts.client, &board.id, "large.bin", &data).await;
    response.check_created();
    let file_id = crate::files::uploaded_id(&response);
    let card: serde_json::Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &serde_json::json!({"title": "Card"}),
        )
        .await
        .created_json();
    let attachment = Block {
        id: new_id(),
        parent_id: card["id"].as_str().expect("card id").to_owned(),
        container_id: board.id.clone(),
        block_type: BlockType::Attachment,
        fields: serde_json::json!({"fileId": file_id})
            .as_object()
            .expect("an object")
            .clone(),
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    ts.client
        .insert_blocks(&board.id, std::slice::from_ref(&attachment))
        .await
        .check_created();

    let exported = export_board(&ts.client, &board).await;
    exported.check_ok();
    let mut zip = zip::ZipArchive::new(Cursor::new(exported.body)).expect("the export is a ZIP");
    let mut entry = zip
        .by_name(&format!("{}/{file_id}", board.id))
        .expect("the file has its entry");
    let mut content = Vec::new();
    entry.read_to_end(&mut content).expect("the entry is read");
    assert!(content == data, "the file is whole in the archive");
}

/// A user runs one export at a time and the server `max_concurrent_exports` (2): over
/// either, an export of a board or of a workspace is 429 with a reason. An export holds
/// its place until it is sent
/// (`app::download`'s tests).
#[tokio::test]
async fn exports_run_one_per_user_and_two_on_the_server() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = board_with_card(&ts, TEST_WORKSPACE_ID).await;
    let me = ts.client.me().await;
    let workspace_export = format!("{TEST_WS}/archive/export");
    let refused = |response: &ApiResponse, reason: &str| {
        response.check_status(StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.error_reason(), reason, "{}", response.text());
    };

    let mine = ts.app.start_export(&me.id).expect("a place for user1");
    refused(&export_board(&ts.client, &board).await, "export_running");
    refused(&ts.client.get(&workspace_export).await, "export_running");
    let other = ts.app.start_export(&new_id()).expect("a place for another");
    refused(
        &ts.client2.get(&workspace_export).await,
        "server_exports_busy",
    );
    // Who may not export the board is told so first: a full server says nothing of it.
    let outsider = export_board(&ts.client2, &board).await;
    assert!(
        [StatusCode::FORBIDDEN, StatusCode::NOT_FOUND].contains(&outsider.status),
        "{}",
        outsider.text()
    );

    drop(mine);
    drop(other);
    export_board(&ts.client, &board).await.check_ok();
    // The place of a sent export is given back.
    until("the place is given back", || {
        ts.app.start_export(&me.id).is_ok()
    })
    .await;
    export_board(&ts.client, &board).await.check_ok();
    ts.client2.get(&workspace_export).await.check_ok();
}

/// A board with three attachments of 6 MiB that do not compress (a request takes 8 MiB at
/// most), so that its archive is larger than what the connection's buffers take in.
async fn board_with_large_archive(ts: &TestServer) -> Container {
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let card: serde_json::Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &serde_json::json!({"title": "Card"}),
        )
        .await
        .created_json();
    let card_id = card["id"].as_str().expect("card id").to_owned();
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let mut attachments = Vec::new();
    for index in 0..3 {
        let data: Vec<u8> = (0..6 * 1024 * 1024)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()[0]
            })
            .collect();
        let response =
            crate::files::upload(&ts.client, &board.id, &format!("noise{index}.bin"), &data).await;
        response.check_created();
        attachments.push(Block {
            id: new_id(),
            parent_id: card_id.clone(),
            container_id: board.id.clone(),
            block_type: BlockType::Attachment,
            fields: serde_json::json!({"fileId": crate::files::uploaded_id(&response)})
                .as_object()
                .expect("an object")
                .clone(),
            created_at: 1,
            updated_at: 1,
            ..Block::default()
        });
    }
    ts.client
        .insert_blocks(&board.id, &attachments)
        .await
        .check_created();
    board
}

/// The temporary archives in the exports directory.
fn temporary_archives(ts: &TestServer) -> usize {
    std::fs::read_dir(ts.app.config().tmp_dir().join("exports")).map_or(0, Iterator::count)
}

/// Asks for a board's archive on a connection of its own that takes little (a small
/// receive buffer, whatever the host's defaults), and reads nothing of the answer.
async fn stalled_download(ts: &TestServer, board_id: &str) -> tokio::net::TcpStream {
    let address: std::net::SocketAddr = ts
        .base_url
        .trim_start_matches("http://")
        .parse()
        .expect("the test server listens on an address");
    let socket = tokio::net::TcpSocket::new_v4().expect("a socket");
    socket
        .set_recv_buffer_size(16 * 1024)
        .expect("a small receive buffer");
    let mut stream = socket.connect(address).await.expect("connected");
    let request = format!(
        "GET /api/v1/w/{TEST_WORKSPACE_ID}/containers/{board_id}/archive/export HTTP/1.1\r\nHost: localhost\r\n\
         Authorization: Bearer {}\r\nX-Requested-With: XMLHttpRequest\r\n\r\n",
        ts.client.token
    );
    tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
        .await
        .expect("the request is sent");
    stream
}

/// Waits for `done`, up to a minute: enough for a machine under any load, and only a
/// test that fails anyway waits that long.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// A client that takes nothing for `export_stall_seconds` has the file removed, then the
/// place given back, and its connection broken short of the archive's end. Only what must
/// happen is waited
/// for, so the outcome does not depend on how busy the machine is; that the place is held
/// while a download goes on is `app::download`'s test, on the paused clock.
#[tokio::test]
async fn a_download_that_takes_nothing_is_given_up() {
    let ts = Setup::native()
        .configure(|config| config.export_stall_seconds = 1)
        .start()
        .await
        .init_basic()
        .await;
    let board = board_with_large_archive(&ts).await;
    let me = ts.client.me().await;

    // The head of the answer comes once the archive is written and its place taken.
    let mut stalled = stalled_download(&ts, &board.id).await;
    let mut head = Vec::new();
    let mut buffer = vec![0; 64 * 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            tokio::io::AsyncReadExt::read(&mut stalled, &mut buffer),
        )
        .await
        .expect("the answer comes")
        .expect("read");
        assert!(read > 0, "the answer has a head");
        head.extend_from_slice(&buffer[..read]);
    }
    let head_text = String::from_utf8_lossy(&head).into_owned();
    assert!(head_text.starts_with("HTTP/1.1 200"), "{head_text}");
    let head_end = head_text.find("\r\n\r\n").expect("the end of the head") + 4;
    let length: usize = head_text
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .expect("the archive's length");

    until("the place is given back", || {
        ts.app.start_export(&me.id).is_ok()
    })
    .await;
    assert_eq!(temporary_archives(&ts), 0, "the file went before the place");

    // What came after the head is of the archive.
    let mut received = head.len() - head_end;
    let ended = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            match tokio::io::AsyncReadExt::read(&mut stalled, &mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => received += read,
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the connection is broken");
    assert!(
        received < length,
        "{received} bytes of an archive of {length}"
    );
}

/// A file a block shows that is not there to be exported, its object gone or its record
/// never made, gets no entry: the board's line lists it in `missingFiles` and leaves it
/// out of `fileNames`. An import stores no file for it, also when an entry of its name
/// was put into the archive after all, and what showed it shows no file: an empty
/// paragraph in a body, the empty name in an attachment.
async fn a_missing_file_is_listed_and_imported_as_missing(ts: &TestServer) {
    let MissingFiles {
        board,
        present,
        gone,
        unrecorded,
    } = board_with_missing_files(ts).await;

    let exported = export_board(&ts.client, &board).await;
    exported.check_ok();
    let archive = zip::ZipArchive::new(Cursor::new(exported.body.clone())).expect("a zip");
    let mut entries: Vec<String> = archive
        .file_names()
        .map(|name| name.expect("an entry's name").into_owned())
        .collect();
    entries.sort_unstable();
    let mut expected = vec![
        "version.json".to_owned(),
        format!("{}/container.jsonl", board.id),
        format!("{}/{present}", board.id),
    ];
    expected.sort_unstable();
    assert_eq!(entries, expected, "no entry for a missing file");
    let line = board_line(archive, &board.id);
    assert_eq!(
        line["data"]["fileNames"],
        serde_json::json!({ present.as_str(): "here.png" }),
        "{line}"
    );
    let mut missing: Vec<String> =
        serde_json::from_value(line["data"]["missingFiles"].clone()).expect("a list");
    missing.sort_unstable();
    let mut expected_missing = vec![gone.clone(), unrecorded.clone()];
    expected_missing.sort_unstable();
    assert_eq!(missing, expected_missing, "{line}");

    // The same archive, and the same with an entry for the missing file put back.
    let with_entry = with_extra_entry(&exported.body, &format!("{}/{gone}", board.id));
    for archive in [exported.body.clone(), with_entry] {
        imported_without(ts, &archive, [gone.as_str(), unrecorded.as_str()]).await;
    }
}

/// A board whose card shows a file there is and one whose object is gone, its record
/// kept, and whose attachment names a file of no record.
struct MissingFiles {
    board: Container,
    present: String,
    gone: String,
    unrecorded: String,
}

async fn board_with_missing_files(ts: &TestServer) -> MissingFiles {
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let upload = async |name: &str, bytes: &[u8]| {
        crate::files::uploaded_id(&crate::files::upload(&ts.client, &board.id, name, bytes).await)
    };
    let present = upload("here.png", b"here").await;
    let gone = upload("gone.png", b"gone").await;
    let gone_info = ts
        .app
        .store()
        .get_file_record(&board.id, &gone)
        .await
        .expect("the record of the file whose object goes");
    ts.app
        .files()
        .delete(&gone_info.path)
        .await
        .expect("its object goes");
    let unrecorded = format!("{}.png", fvoci_server::auth::secret::new_secret());
    let body = serde_json::json!({"type": "doc", "content": [
        {"type": "image", "attrs": {"fileId": present}},
        {"type": "image", "attrs": {"fileId": gone}},
    ]});
    let card: serde_json::Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &serde_json::json!({"title": "Card", "body": body}),
        )
        .await
        .created_json();
    let mut fields = fvoci_model::JsonMap::new();
    fields.insert("fileId".to_owned(), serde_json::json!(unrecorded));
    let attachment = Block {
        id: new_id(),
        parent_id: card["id"].as_str().expect("card id").to_owned(),
        container_id: board.id.clone(),
        block_type: BlockType::Attachment,
        fields,
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    ts.client
        .insert_blocks(&board.id, std::slice::from_ref(&attachment))
        .await
        .check_created();

    MissingFiles {
        board,
        present,
        gone,
        unrecorded,
    }
}

/// Imports `archive` of [`board_with_missing_files`] and checks the new board: the
/// present file is shown and there, the missing ones neither shown nor recorded.
async fn imported_without(ts: &TestServer, archive: &[u8], missing: [&str; 2]) {
    let before: Vec<String> = boards_of(ts).await.into_iter().map(|b| b.id).collect();
    import(&ts.client, TEST_WORKSPACE_ID, archive)
        .await
        .check_ok();
    let imported = boards_of(ts)
        .await
        .into_iter()
        .find(|other| !before.contains(&other.id))
        .expect("the imported board");
    let cards: Vec<serde_json::Value> = ts
        .client
        .get(&format!(
            "{TEST_WS}/boards/{}/blocks?type=card",
            imported.id
        ))
        .await
        .ok_json();
    let content = &cards[0]["body"]["content"];
    assert_eq!(content[0]["type"], "image", "the present file: {content}");
    let new_present = content[0]["attrs"]["fileId"].as_str().expect("a file");
    ts.client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{new_present}",
            imported.id
        ))
        .await
        .check_ok();
    assert_eq!(
        content[1]["type"], "paragraph",
        "the missing file: {content}"
    );
    let attachments: Vec<serde_json::Value> = ts
        .client
        .get(&format!(
            "{TEST_WS}/boards/{}/blocks?type=attachment",
            imported.id
        ))
        .await
        .ok_json();
    assert_eq!(
        attachments[0]["fields"]["fileId"],
        serde_json::json!(""),
        "{attachments:?}"
    );
    for name in missing {
        let missing = ts.app.store().get_file_record(&imported.id, name).await;
        assert!(missing.is_err(), "no record of {name}: {missing:?}");
    }
}

/// `archive` with one more entry, `name`, holding a few bytes.
fn with_extra_entry(archive: &[u8], name: &str) -> Vec<u8> {
    let mut source = zip::ZipArchive::new(Cursor::new(archive.to_vec())).expect("a zip");
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let entry = source.by_index_raw(index).expect("an entry");
        writer.raw_copy_file(entry).expect("copied");
    }
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file(name, options).expect("start");
    writer.write_all(b"not the file").expect("write");
    writer.finish().expect("finish").into_inner()
}

#[tokio::test]
async fn a_missing_file_is_listed_in_the_archive_and_imported_as_missing() {
    let ts = Setup::native().start().await.init_basic().await;
    a_missing_file_is_listed_and_imported_as_missing(&ts).await;
    ts.teardown().await;
}

#[tokio::test]
#[ignore = "needs an S3-compatible server; see tests/storage.rs"]
async fn with_s3_a_missing_file_is_listed_in_the_archive_and_imported_as_missing() {
    let ts = crate::uploads::s3_server().await;
    a_missing_file_is_listed_and_imported_as_missing(&ts).await;
    ts.teardown().await;
}
