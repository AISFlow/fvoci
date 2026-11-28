//! Imports through an upload in parts: the archive
//! is sent as a file is, imported by naming the upload, held to the limits of an
//! archive, and nothing of the upload stays behind.

use std::io::{Cursor, Write as _};
use std::path::PathBuf;

use fvoci_model::{Block, BlockType, ContainerVisibility, new_id};
use fvoci_server::storage::FaultyOp;
use fvoci_store::LooseState;
use reqwest::StatusCode;
use serde_json::json;

use crate::common::TEST_WS;
use crate::common::{Client, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TestServer};
use crate::export::{boards_of, import, import_upload, put_part};

/// The smallest part size, so that an archive of two parts stays small.
const PART: usize = 5 * 1024 * 1024;

/// The largest `container.jsonl` an import reads.
const JSONL_LIMIT: usize = 64 * 1024 * 1024;

/// The directory under which the parts of uploads are kept.
fn uploads_dir(ts: &TestServer) -> PathBuf {
    ts.app.config().tmp_dir().join("uploads")
}

/// What is in a directory, by name; nothing if it is not there.
fn entries(directory: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(directory).map_or_default(|read| {
        read.map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
    })
}

/// Opens an upload of an archive of `size` bytes into `workspace_id`.
async fn open_import(
    client: &Client,
    workspace_id: &str,
    size: usize,
) -> crate::common::ApiResponse {
    client
        .post(
            &format!("/w/{workspace_id}/archive/uploads"),
            &json!({"name": "archive.fvoci", "size": size}),
        )
        .await
}

/// The identifier of an upload that was opened.
fn upload_id(response: &crate::common::ApiResponse) -> String {
    response.check_created();
    response.value()["uploadId"]
        .as_str()
        .expect("an upload has an identifier")
        .to_owned()
}

/// Bytes that do not compress, so that an archive holding them is as large as they are.
fn incompressible(size: usize) -> Vec<u8> {
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect()
}

/// An archive of version 1 with one board and the given entries beside its
/// `container.jsonl`, each written with `method`.
fn archive_with(entries: &[(&str, &[u8])], method: zip::CompressionMethod) -> Vec<u8> {
    let board = json!({"type": "container", "data": {
        "id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Limits", "createdAt": 1, "updatedAt": 1,
    }});
    archive_of(&board, entries, method)
}

/// An archive of version 1 with one board, the line `board` of its `container.jsonl`, and
/// the given entries beside it.
fn archive_of(
    board: &serde_json::Value,
    entries: &[(&str, &[u8])],
    method: zip::CompressionMethod,
) -> Vec<u8> {
    let options = zip::write::SimpleFileOptions::default().compression_method(method);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("version.json", options).expect("start");
    writer
        .write_all(br#"{"version":1,"date":1}"#)
        .expect("write");
    writer
        .start_file("b1/container.jsonl", options)
        .expect("start");
    writer
        .write_all(format!("{board}\n").as_bytes())
        .expect("write");
    for (name, content) in entries {
        writer.start_file(*name, options).expect("start");
        writer.write_all(content).expect("write");
    }
    writer.finish().expect("finish").into_inner()
}

/// An archive of two parts is imported whole, and its upload leaves nothing on the
/// disk.
#[tokio::test]
async fn an_archive_in_parts_is_imported_and_leaves_nothing() {
    let ts = Setup::native()
        .configure(|config| config.upload_part_bytes = 5 * 1024 * 1024)
        .start()
        .await
        .init_basic()
        .await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    // A file is at most a part when sent whole: two of them make an archive of two.
    let data = incompressible(PART);
    let mut attachments = Vec::new();
    let card: serde_json::Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Card"}),
        )
        .await
        .created_json();
    let card_id = card["id"].as_str().expect("card id").to_owned();
    for half in data.chunks(PART / 2 + 1024) {
        let response = crate::files::upload(&ts.client, &board.id, "half.bin", half).await;
        response.check_created();
        let file_id = crate::files::uploaded_id(&response);
        attachments.push(Block {
            id: new_id(),
            parent_id: card_id.clone(),
            container_id: board.id.clone(),
            block_type: BlockType::Attachment,
            fields: json!({"fileId": file_id})
                .as_object()
                .expect("an object")
                .clone(),
            created_at: 1,
            updated_at: 1,
            ..Block::default()
        });
    }
    crate::blocks::insert(&ts.client, &board.id, &attachments).await;
    let archive = ts
        .client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", board.id))
        .await
        .body;
    assert!(archive.len() > PART, "the archive comes in two parts");

    let before = boards_of(&ts).await.len();
    import(&ts.client, TEST_WORKSPACE_ID, &archive)
        .await
        .check_ok();
    assert_eq!(boards_of(&ts).await.len(), before + 1, "the board is made");
    assert_eq!(
        entries(&uploads_dir(&ts)),
        Vec::<String>::new(),
        "nothing stays"
    );

    // An import that fails leaves nothing either.
    import(&ts.client, TEST_WORKSPACE_ID, b"not an archive")
        .await
        .check_bad_request();
    assert_eq!(
        entries(&uploads_dir(&ts)),
        Vec::<String>::new(),
        "nothing stays"
    );
    ts.teardown().await;
}

/// A board in an archive names its workspace with `workspaceId`: it is imported into the
/// target workspace whatever `workspaceId` the archive names, refused when `workspaceId`
/// is not a string, and unknown keys are ignored rather than mapped to `workspaceId`.
#[tokio::test]
async fn an_archive_board_validates_workspace_id_and_ignores_unknown_keys() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = json!({"type": "container", "data": {
        "id": "b1", "workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": "Imported board",
        "createdAt": 1, "updatedAt": 1,
    }});
    let archive = archive_of(&board, &[], zip::CompressionMethod::Deflated);
    import(&ts.client, TEST_WORKSPACE_ID, &archive)
        .await
        .check_ok();
    let imported: Vec<_> = boards_of(&ts)
        .await
        .into_iter()
        .filter(|board| board.title == "Imported board")
        .collect();
    assert_eq!(imported.len(), 1, "{imported:?}");
    assert_eq!(imported[0].workspace_id, TEST_WORKSPACE_ID);

    let invalid = json!({"type": "container", "data": {
        "id": "b2", "workspaceId": 5, "visibility": "private", "title": "Numbered", "createdAt": 1, "updatedAt": 1,
    }});
    let archive = archive_of(&invalid, &[], zip::CompressionMethod::Deflated);
    let refused = import(&ts.client, TEST_WORKSPACE_ID, &archive).await;
    refused.check_bad_request();
    let detail = refused.value()["detail"].as_str().unwrap_or("").to_owned();
    assert!(
        detail.starts_with("invalid container in archive line 1"),
        "workspaceId: {detail}"
    );

    // An unknown workspace-like key is ignored rather than aliased to `workspaceId`.
    let unknown_key = json!({"type": "container", "data": {
        "id": "b3", "legacyWorkspaceId": 5, "visibility": "private", "title": "Unknown key ignored",
        "createdAt": 1, "updatedAt": 1,
    }});
    let archive = archive_of(&unknown_key, &[], zip::CompressionMethod::Deflated);
    import(&ts.client, TEST_WORKSPACE_ID, &archive)
        .await
        .check_ok();
    let imported: Vec<_> = boards_of(&ts)
        .await
        .into_iter()
        .filter(|board| board.title == "Unknown key ignored")
        .collect();
    assert_eq!(imported.len(), 1, "{imported:?}");
    assert_eq!(imported[0].workspace_id, TEST_WORKSPACE_ID);
    ts.teardown().await;
}

/// A board of an archive that says it is a wiki is made a wiki; one that the store would
/// not take (a wiki that is a template) is refused, and nothing of the archive is
/// stored, the boards before it neither. The kind `board`, which an export writes, is
/// taken.
#[tokio::test]
async fn an_archive_makes_its_wiki_a_wiki_and_refuses_a_template_wiki_whole() {
    let ts = Setup::native().start().await.init_basic().await;
    let board_of = |kind: &str, template: bool| {
        json!({"type": "container", "data": {
            "id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "kind": kind,
            "isTemplate": template, "title": format!("Archived {kind}"), "createdAt": 1,
            "updatedAt": 1,
        }})
    };
    let archive = |kind: &str| {
        archive_of(
            &board_of(kind, false),
            &[],
            zip::CompressionMethod::Deflated,
        )
    };
    // A board first and the wiki after it: the board is not stored before the refusal.
    let wiki_line = format!("{}\n", board_of("wiki", true)).into_bytes();
    let board_then_wiki = archive_of(
        &board_of("board", false),
        &[("b2/container.jsonl", &wiki_line)],
        zip::CompressionMethod::Deflated,
    );
    let refused = import(&ts.client, TEST_WORKSPACE_ID, &board_then_wiki).await;
    refused.check_bad_request();
    // It is said of the archive, and of no member of the request, which has none of it.
    assert_eq!(refused.refused_place(), None, "{}", refused.text());
    assert!(
        !boards_of(&ts)
            .await
            .iter()
            .any(|board| board.title.starts_with("Archived")),
        "a board of a refused archive is stored"
    );
    import(&ts.client, TEST_WORKSPACE_ID, &archive("board"))
        .await
        .check_ok();
    import(&ts.client, TEST_WORKSPACE_ID, &archive("wiki"))
        .await
        .check_ok();
    let titles: Vec<String> = boards_of(&ts)
        .await
        .into_iter()
        .map(|board| board.title)
        .filter(|title| title.starts_with("Archived"))
        .collect();
    assert_eq!(titles, ["Archived board"]);
    let wikis = ts.client.get(&format!("{TEST_WS}/wikis")).await.value();
    let wikis = wikis.as_array().expect("a list");
    assert_eq!(wikis.len(), 1);
    assert_eq!(wikis[0]["title"], json!("Archived wiki"));
    assert_eq!(wikis[0]["kind"], json!("wiki"));
    ts.teardown().await;
}

/// An upload of an archive is held to what an import takes, and is imported only into
/// its workspace, by `importArchive`; one of a file is completed only by `completeUpload`.
#[tokio::test]
async fn an_upload_of_an_archive_is_kept_to_its_purpose() {
    let ts = Setup::native()
        .configure(|config| config.max_import_bytes = 1000)
        .start()
        .await
        .init_basic()
        .await;
    let config = ts.client.get("/client-config").await;
    assert_eq!(config.value()["maxImportBytes"], json!(1000));

    let response = open_import(&ts.client, TEST_WORKSPACE_ID, 1001).await;
    response.check_status(StatusCode::PAYLOAD_TOO_LARGE);
    let problem = response.value();
    assert_eq!(
        (&problem["type"], &problem["reason"], &problem["limit"]),
        (
            &json!("/problems/archive-too-large"),
            &json!("archive_too_large"),
            &json!(1000)
        ),
        "{problem}"
    );

    let archive = archive_with(&[], zip::CompressionMethod::Stored);
    let id = upload_id(&open_import(&ts.client, TEST_WORKSPACE_ID, archive.len()).await);
    let state = ts.client.get(&format!("{TEST_WS}/uploads/{id}")).await;
    assert_eq!(state.value()["purpose"], json!("import"));
    assert_eq!(state.value()["workspaceId"], json!(TEST_WORKSPACE_ID));
    assert_eq!(state.value()["containerId"], json!(""));

    // A part missing: refused, and the upload stays open for it.
    let response = import_upload(&ts.client, TEST_WORKSPACE_ID, &id).await;
    response.check_bad_request();
    put_part(&ts.client, TEST_WORKSPACE_ID, &id, 1, &archive)
        .await
        .check_status(StatusCode::NO_CONTENT);
    let response = ts
        .client
        .post(&format!("{TEST_WS}/uploads/{id}/complete"), &json!({}))
        .await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "parameter uploadId", "of the path");
    // Under another workspace the upload is not there, as one that does not exist.
    let response = import_upload(&ts.client, ROOT_WORKSPACE_ID, &id).await;
    response.check_not_found();
    assert_eq!(response.error_message(), "upload not found");
    import_upload(&ts.client2, TEST_WORKSPACE_ID, &id)
        .await
        .check_status(StatusCode::NOT_FOUND);

    import_upload(&ts.client, TEST_WORKSPACE_ID, &id)
        .await
        .check_ok();
    import_upload(&ts.client, TEST_WORKSPACE_ID, &id)
        .await
        .check_status(StatusCode::NOT_FOUND);

    // An upload of a file is not imported.
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = ts
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/uploads", board.id),
            &json!({"name": "a.bin", "size": 3}),
        )
        .await;
    let file_upload = upload_id(&response);
    let response = import_upload(&ts.client, TEST_WORKSPACE_ID, &file_upload).await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "/uploadId");
    ts.teardown().await;
}

/// An archive over a limit of what it unpacks to is 413 with the limit, and nothing of
/// it is stored. An entry that is too large says so by its reason; the count of the
/// entries has none.
#[tokio::test]
async fn an_archive_over_its_limits_stores_nothing() {
    let ts = Setup::native()
        .configure(|config| config.max_file_bytes = 1000)
        .start()
        .await
        .init_basic()
        .await;
    let before = boards_of(&ts).await.len();

    let file = vec![7; 1001];
    let too_large_file = archive_with(&[("b1/large.bin", &file)], zip::CompressionMethod::Deflated);
    let response = import(&ts.client, TEST_WORKSPACE_ID, &too_large_file).await;
    response.check_status(StatusCode::PAYLOAD_TOO_LARGE);
    let problem = response.check_problem();
    assert_eq!(
        (&problem["type"], &problem["reason"], &problem["limit"]),
        (
            &json!("/problems/archive-entry-too-large"),
            &json!("archive_entry_too_large"),
            &json!(1000)
        ),
        "{problem}"
    );

    let names: Vec<String> = (0..10_000).map(|index| format!("b1/{index}.bin")).collect();
    let many: Vec<(&str, &[u8])> = names.iter().map(|name| (name.as_str(), &b""[..])).collect();
    let too_many = archive_with(&many, zip::CompressionMethod::Stored);
    let response = import(&ts.client, TEST_WORKSPACE_ID, &too_many).await;
    response.check_status(StatusCode::PAYLOAD_TOO_LARGE);
    let problem = response.check_problem();
    assert_eq!(problem["limit"], json!(10_000));
    assert_eq!(problem.get("reason"), None, "{problem}");

    // An end record that declares fewer entries than the archive has changes nothing:
    // the entries are counted by their headers before the directory is read.
    let mut forged = too_many.clone();
    let end = forged
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .expect("an end record");
    forged[end + 8..end + 12].copy_from_slice(&[5, 0, 5, 0]);
    let response = import(&ts.client, TEST_WORKSPACE_ID, &forged).await;
    response.check_status(StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.value()["limit"], json!(10_000));

    let jsonl = archive_with(&[], zip::CompressionMethod::Deflated);
    let mut zip = zip::ZipArchive::new(Cursor::new(jsonl)).expect("a ZIP");
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .raw_copy_file(zip.by_name("version.json").expect("the version"))
        .expect("copy");
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer
        .start_file("b1/container.jsonl", deflated)
        .expect("start");
    writer
        .write_all(&vec![b' '; JSONL_LIMIT + 1])
        .expect("write");
    let large_jsonl = writer.finish().expect("finish").into_inner();
    let response = import(&ts.client, TEST_WORKSPACE_ID, &large_jsonl).await;
    response.check_status(StatusCode::PAYLOAD_TOO_LARGE);
    let problem = response.check_problem();
    assert_eq!(
        (&problem["reason"], &problem["limit"]),
        (&json!("archive_entry_too_large"), &json!(JSONL_LIMIT)),
        "{problem}"
    );

    assert_eq!(boards_of(&ts).await.len(), before, "no board was made");
    assert_eq!(
        entries(&uploads_dir(&ts)),
        Vec::<String>::new(),
        "nothing stays"
    );
    ts.teardown().await;
}

/// What an import that gave up leaves: no board, no object marked as being written, and
/// no staging directory.
async fn assert_given_up(ts: &TestServer, boards_before: usize, why: &str) {
    assert_eq!(boards_of(ts).await.len(), boards_before, "no board ({why})");
    let writing: Vec<_> = ts
        .app
        .housekeeping()
        .store()
        .loose_objects()
        .await
        .expect("listed")
        .into_iter()
        .filter(|object| object.state == LooseState::Writing)
        .collect();
    assert_eq!(writing, [], "nothing is being written ({why})");
    let staging: Vec<String> = entries(&ts.app.config().tmp_dir())
        .into_iter()
        .filter(|name| name.starts_with("import-stage-"))
        .collect();
    assert_eq!(
        staging,
        Vec::<String>::new(),
        "no staging directory ({why})"
    );
}

/// An import whose second file cannot be written, or would take the organization past
/// its storage, gives up the first one too, and leaves nothing.
#[tokio::test]
async fn an_import_whose_second_file_fails_leaves_nothing() {
    let ts = Setup::native()
        .configure(|config| {
            config.max_file_bytes = 1000;
            config.max_org_storage_bytes = 1500;
        })
        .start()
        .await
        .init_basic()
        .await;
    let before = boards_of(&ts).await.len();

    let small = incompressible(400);
    let two = archive_with(
        &[("b1/first.bin", &small), ("b1/second.bin", &small)],
        zip::CompressionMethod::Stored,
    );
    let faults = ts
        .app
        .files()
        .faults()
        .expect("a test's storage takes faults");
    faults.fail_after(FaultyOp::Write, 1, 1);
    let response = import(&ts.client, TEST_WORKSPACE_ID, &two).await;
    assert!(response.status.is_server_error(), "{}", response.text());
    assert_given_up(&ts, before, "a write failed").await;

    let large = incompressible(800);
    let past_the_room = archive_with(
        &[("b1/first.bin", &large), ("b1/second.bin", &large)],
        zip::CompressionMethod::Stored,
    );
    let response = import(&ts.client, TEST_WORKSPACE_ID, &past_the_room).await;
    response.check_status(StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.check_problem()["reason"], json!("storage_full"));
    assert_given_up(&ts, before, "past the storage").await;
    ts.teardown().await;
}

/// The open uploads of a user hold back at most twice the larger of the two limits, a
/// file's and an archive's, and an archive counts toward it as a file does.
#[tokio::test]
async fn the_share_of_a_user_is_twice_the_larger_limit() {
    let ts = Setup::native()
        .configure(|config| {
            config.max_file_bytes = 1000;
            config.max_import_bytes = 2000;
            config.max_pending_upload_bytes = 100_000;
        })
        .start()
        .await
        .init_basic()
        .await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let open_file = |size: usize| {
        let path = format!("{TEST_WS}/containers/{}/uploads", board.id);
        let client = &ts.client;
        async move {
            client
                .post(&path, &json!({"name": "a.bin", "size": size}))
                .await
        }
    };
    open_file(1000).await.check_created();
    open_file(1000).await.check_created();
    open_import(&ts.client, TEST_WORKSPACE_ID, 2000)
        .await
        .check_created();
    let response = open_file(1).await;
    response.check_status(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.error_reason(), "uploads_pending");
    let response = open_import(&ts.client, TEST_WORKSPACE_ID, 1).await;
    response.check_status(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.error_reason(), "uploads_pending");
    ts.teardown().await;
}

/// With the files in S3, an archive's parts are still kept on the server's disk (the
/// import reads it at random), and its files go to the bucket.
#[tokio::test]
#[ignore = "needs an S3-compatible server: set FVOCI_S3_TEST_ENDPOINT and FVOCI_S3_TEST_BUCKET"]
async fn with_s3_an_archive_is_received_on_the_disk() {
    let ts = crate::uploads::s3_server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let card: serde_json::Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Card"}),
        )
        .await
        .created_json();
    let response = crate::files::upload(&ts.client, &board.id, "a.bin", b"in the bucket").await;
    response.check_created();
    let file_id = crate::files::uploaded_id(&response);
    let attachment = Block {
        id: new_id(),
        parent_id: card["id"].as_str().expect("card id").to_owned(),
        container_id: board.id.clone(),
        block_type: BlockType::Attachment,
        fields: json!({"fileId": file_id})
            .as_object()
            .expect("an object")
            .clone(),
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    crate::blocks::insert_one(&ts.client, &attachment).await;
    let archive = ts
        .client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", board.id))
        .await
        .body;

    let id = upload_id(&open_import(&ts.client, TEST_WORKSPACE_ID, archive.len()).await);
    let opened = ts
        .app
        .store()
        .uploads_to_clean_up(i64::MAX, 0)
        .await
        .expect("the uploads are read")
        .into_iter()
        .find(|upload| upload.id == id)
        .expect("the upload is open");
    assert_eq!(opened.multipart_id, "", "no multipart upload in the bucket");
    put_part(&ts.client, TEST_WORKSPACE_ID, &id, 1, &archive)
        .await
        .check_status(StatusCode::NO_CONTENT);
    assert_eq!(
        entries(&uploads_dir(&ts).join(&id)).len(),
        1,
        "the part is on the disk"
    );

    let before = boards_of(&ts).await.len();
    import_upload(&ts.client, TEST_WORKSPACE_ID, &id)
        .await
        .check_ok();
    let boards = boards_of(&ts).await;
    assert_eq!(boards.len(), before + 1, "the board is made");
    assert_eq!(
        entries(&uploads_dir(&ts)),
        Vec::<String>::new(),
        "nothing stays"
    );
    ts.teardown().await;
}

/// A `version.json` at the root that is too large to be one is no archive of this
/// server's (400); one inside a board's directory is a file like any other.
#[tokio::test]
async fn a_large_version_json_is_no_archive() {
    let ts = Setup::native().start().await.init_basic().await;
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut version = br#"{"version":1,"date":1}"#.to_vec();
    version.resize(65 * 1024, b' ');
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("version.json", options).expect("start");
    writer.write_all(&version).expect("write");
    let large_root = writer.finish().expect("finish").into_inner();
    let response = import(&ts.client, TEST_WORKSPACE_ID, &large_root).await;
    response.check_bad_request();

    let nested = archive_with(
        &[("b1/version.json", &version)],
        zip::CompressionMethod::Deflated,
    );
    import(&ts.client, TEST_WORKSPACE_ID, &nested)
        .await
        .check_ok();
    ts.teardown().await;
}

/// A user runs one import at a time and the server `max_concurrent_imports` (2): over
/// either, the import is 429 with a reason, and the upload stays to be imported again.
#[tokio::test]
async fn imports_run_one_per_user_and_two_on_the_server() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let archive = ts
        .client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", board.id))
        .await;
    archive.check_ok();
    let me = ts.client.me().await;
    let opened = upload_id(&open_import(&ts.client, ROOT_WORKSPACE_ID, archive.body.len()).await);
    put_part(&ts.client, ROOT_WORKSPACE_ID, &opened, 1, &archive.body)
        .await
        .check_no_content();

    let mine = ts.app.start_import(&me.id).expect("a place for user1");
    let response = import_upload(&ts.client, ROOT_WORKSPACE_ID, &opened).await;
    response.check_status(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.error_reason(), "import_running");
    drop(mine);
    let first = ts.app.start_import(&new_id()).expect("a place");
    let second = ts.app.start_import(&new_id()).expect("another place");
    let response = import_upload(&ts.client, ROOT_WORKSPACE_ID, &opened).await;
    response.check_status(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.error_reason(), "server_imports_busy");
    // Another user's upload is not there for whoever names it, full server or not.
    import_upload(&ts.client2, ROOT_WORKSPACE_ID, &opened)
        .await
        .check_not_found();

    drop(first);
    drop(second);
    import_upload(&ts.client, ROOT_WORKSPACE_ID, &opened)
        .await
        .check_ok();
    ts.teardown().await;
}

/// Native archives with duplicate `(dir, "container.jsonl")` or duplicate `(dir, filename)`
/// entries are rejected with 400 before staging any files or creating boards.
#[tokio::test]
async fn duplicate_entries_in_native_archive_are_refused_with_400() {
    let ts = Setup::native().start().await.init_basic().await;
    let before = boards_of(&ts).await.len();
    let board = json!({"type": "container", "data": {
        "id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Dup", "createdAt": 1, "updatedAt": 1,
    }});
    let opts = zip::write::SimpleFileOptions::default();

    // Case 1: duplicate `(dir, "container.jsonl")` via normalized path alias.
    let mut w1 = zip::ZipWriter::new(Cursor::new(Vec::new()));
    w1.start_file("version.json", opts).expect("start");
    w1.write_all(br#"{"version":1,"date":1}"#).expect("write");
    for path in ["b1/container.jsonl", "./b1/container.jsonl"] {
        w1.start_file(path, opts).expect("start");
        w1.write_all(format!("{board}\n").as_bytes())
            .expect("write");
    }
    let dup_jsonl = w1.finish().expect("finish").into_inner();
    import(&ts.client, TEST_WORKSPACE_ID, &dup_jsonl)
        .await
        .check_bad_request();

    // Case 2: duplicate `(dir, "photo.png")` via normalized path alias.
    let dup_file = archive_of(
        &board,
        &[("b1/photo.png", b"one"), ("./b1/photo.png", b"two")],
        zip::CompressionMethod::Deflated,
    );
    import(&ts.client, TEST_WORKSPACE_ID, &dup_file)
        .await
        .check_bad_request();

    assert_eq!(boards_of(&ts).await.len(), before, "zero boards created");
    assert_eq!(
        ts.app.store().loose_objects().await.expect("loose"),
        Vec::new(),
        "zero orphan staged objects"
    );
    ts.teardown().await;
}

fn oversized_after_rename_native_zip() -> Vec<u8> {
    // Build a valid body (~4,194,007 bytes <= 4 MiB) with 1 text node + 12 short-ID images
    // (`"1.png"` -> 47-char `<secret>.png`, adding +504 bytes > 4 MiB after rename!).
    let big_text = "a".repeat(fvoci_model::BODY_MAX_BYTES - 900);
    let mut content = vec![json!({
        "type": "paragraph",
        "content": [{ "type": "text", "text": big_text }]
    })];
    for _ in 0..12 {
        content.push(json!({ "type": "image", "attrs": { "fileId": "1.png" } }));
    }
    let doc = json!({ "type": "doc", "content": content });
    assert!(
        fvoci_model::Body::parse(doc.clone()).is_ok(),
        "pre-rename body fits within BODY_MAX_BYTES"
    );

    let b1 = json!({"type": "container", "data": {
        "id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Valid First", "createdAt": 1, "updatedAt": 1,
    }});
    let b2 = json!({"type": "container", "data": {
        "id": "b2", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Overflow Second", "createdAt": 1, "updatedAt": 1,
    }});
    let card = json!({"type": "block", "data": {
        "id": "c1", "containerId": "b2", "parentId": "", "type": "card", "title": "Big",
        "fields": {}, "body": doc, "createdAt": 1, "updatedAt": 1,
    }});
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("version.json", opts).expect("v");
    writer.write_all(br#"{"version":1,"date":1}"#).expect("v");
    writer.start_file("b1/container.jsonl", opts).expect("b1");
    writer.write_all(format!("{b1}\n").as_bytes()).expect("b1");
    writer.start_file("b2/container.jsonl", opts).expect("b2");
    writer
        .write_all(format!("{b2}\n{card}\n").as_bytes())
        .expect("b2");
    writer.start_file("b2/1.png", opts).expect("png");
    writer.write_all(b"\x89PNG\r\n\x1a\n").expect("png");
    writer.finish().expect("finish").into_inner()
}

/// When renaming short placeholder file IDs or cross-board references pushes a block's
/// serialized body past `BODY_MAX_BYTES`, native import and staged Notion import reject
/// with 400, roll back all boards in the transaction, and clean up staged files.
#[tokio::test]
async fn body_exceeding_max_bytes_after_file_rename_is_rejected_and_rolled_back() {
    let ts = Setup::native().start().await.init_basic().await;
    let before = boards_of(&ts).await.len();
    let native_zip = oversized_after_rename_native_zip();

    let resp = import(&ts.client, TEST_WORKSPACE_ID, &native_zip).await;
    resp.check_bad_request();
    assert_eq!(
        boards_of(&ts).await.len(),
        before,
        "multi-board transaction rolled back completely"
    );
    let doomed_only = ts
        .app
        .store()
        .loose_objects()
        .await
        .expect("loose")
        .into_iter()
        .all(|obj| obj.attempts == 0);
    assert!(doomed_only, "staged native files cleaned up on rollback");

    // Staged Notion ZIP import: Markdown page near 4 MiB with local images (`asset00001.png` -> 47 chars).
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let md_images = "![](p.png)\n\n".repeat(20);
    let md_text = "b".repeat(fvoci_model::BODY_MAX_BYTES - 1500);
    let md = format!("# Big Page\n\n{md_text}\n\n{md_images}");
    let mut nw = zip::ZipWriter::new(Cursor::new(Vec::new()));
    nw.start_file("Page 11111111111111111111111111111111.md", opts)
        .expect("md");
    nw.write_all(md.as_bytes()).expect("md");
    nw.start_file("p.png", opts).expect("png");
    nw.write_all(b"\x89PNG\r\n\x1a\n").expect("png");
    let notion_zip = nw.finish().expect("finish").into_inner();
    let importer_id = ts.client.me().await.id;
    let upload = ts
        .app
        .create_import_upload(
            &importer_id,
            TEST_WORKSPACE_ID,
            fvoci_server::app::FileToUpload {
                name: "notion.zip".to_owned(),
                size: i64::try_from(notion_zip.len()).expect("size"),
                content_type: "application/zip".to_owned(),
            },
        )
        .await
        .expect("upload created");
    put_part(&ts.client, TEST_WORKSPACE_ID, &upload.id, 1, &notion_zip)
        .await
        .check_status(StatusCode::NO_CONTENT);
    let err = ts
        .app
        .import_upload_with_format(&upload, fvoci_server::app::ImportFormat::Notion)
        .await
        .expect_err("staged Notion import exceeding BODY_MAX_BYTES after rename rejected");
    assert_eq!(err.kind(), fvoci_server::error::ErrorKind::BadRequest);
    let wikis: Vec<serde_json::Value> = ts
        .client
        .get(&format!("/w/{TEST_WORKSPACE_ID}/wikis"))
        .await
        .ok_json();
    assert!(wikis.is_empty(), "staged wiki rolled back");
    assert_eq!(
        boards_of(&ts).await.len(),
        before,
        "staged import rolled back"
    );

    ts.teardown().await;
}

fn native_archive_entries(entries: &[(&str, &str)]) -> Vec<u8> {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("version.json", options).expect("start");
    writer
        .write_all(br#"{"version":1,"date":1}"#)
        .expect("write");
    for (name, content) in entries {
        writer.start_file(*name, options).expect("start");
        writer.write_all(content.as_bytes()).expect("write");
    }
    writer.finish().expect("finish").into_inner()
}

/// `container × containerMember` amplification (>64 containers per `container.jsonl`
/// or >10,000 member lines) is rejected immediately with 413 `archive_too_large` and
/// leaves zero DB rows, files, or upload temp residue.
#[tokio::test]
async fn native_archive_container_member_amplification_is_rejected_with_413_and_zero_residue() {
    let ts = Setup::native().start().await.init_basic().await;
    let user2_id = ts.client2.me().await.id;
    let before_boards = boards_of(&ts).await.len();
    let before_files = ts.app.store().count_rows("files").await.expect("files");

    // 1. 65 container lines in one `container.jsonl` (> MAX_CONTAINERS_PER_ENTRY = 64) -> 413.
    let mut lines_65 = String::new();
    for idx in 0..65 {
        let line = json!({
            "type": "container",
            "data": {"id": format!("b{idx}"), "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Amplify", "createdAt": 1, "updatedAt": 1}
        });
        lines_65.push_str(&line.to_string());
        lines_65.push('\n');
    }
    let too_many_containers = native_archive_entries(&[("b0/container.jsonl", &lines_65)]);
    let r1 = import(&ts.client, TEST_WORKSPACE_ID, &too_many_containers).await;
    assert_eq!(r1.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r1.text());
    assert_eq!(
        r1.error_reason(),
        "archive_too_large",
        "container amplification reason"
    );

    // 2. 10,001 `containerMember` lines in one `container.jsonl` (> MAX_MEMBER_LINES = 10_000) -> 413.
    let mut many_members = json!({
        "type": "container",
        "data": {"id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Members", "createdAt": 1, "updatedAt": 1}
    })
    .to_string();
    many_members.push('\n');
    let member_line = json!({
        "type": "containerMember",
        "data": {"containerId": "b1", "userId": user2_id, "role": "editor"}
    })
    .to_string();
    for _ in 0..10_001 {
        many_members.push_str(&member_line);
        many_members.push('\n');
    }
    let too_many_members = native_archive_entries(&[("b1/container.jsonl", &many_members)]);
    let r2 = import(&ts.client, TEST_WORKSPACE_ID, &too_many_members).await;
    assert_eq!(r2.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r2.text());
    assert_eq!(
        r2.error_reason(),
        "archive_too_large",
        "member line amplification reason"
    );

    assert_eq!(
        boards_of(&ts).await.len(),
        before_boards,
        "zero containers created"
    );
    assert_eq!(
        ts.app.store().count_rows("files").await.expect("files"),
        before_files,
        "zero files created"
    );
    assert!(
        entries(&uploads_dir(&ts)).is_empty(),
        "zero upload temp files"
    );
    assert!(
        ts.app
            .store()
            .loose_objects()
            .await
            .expect("loose")
            .is_empty(),
        "zero loose_objects residue"
    );
    ts.teardown().await;
}

/// 1,000 duplicate `containerMember` lines deduplicate to a single membership (`200 OK`),
/// normal export/import round-trips cleanly, and a view of a version 1 archive with a
/// key that no view has is rejected with `400 Bad Request`.
#[tokio::test]
async fn native_archive_deduplicates_members_round_trips_and_rejects_retired_v1_view_keys() {
    let ts = Setup::native().start().await.init_basic().await;
    let user2_id = ts.client2.me().await.id;

    // 1,000 duplicate `containerMember` lines for `user2_id` -> 200 OK with 1 membership for user2.
    let mut dup_jsonl = json!({
        "type": "container",
        "data": {"id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Deduped Board", "createdAt": 1, "updatedAt": 1}
    })
    .to_string();
    dup_jsonl.push('\n');
    let dup_member = json!({
        "type": "containerMember",
        "data": {"containerId": "b1", "userId": user2_id, "role": "editor"}
    })
    .to_string();
    for _ in 0..1_000 {
        dup_jsonl.push_str(&dup_member);
        dup_jsonl.push('\n');
    }
    let dup_archive = native_archive_entries(&[("b1/container.jsonl", &dup_jsonl)]);
    import(&ts.client, TEST_WORKSPACE_ID, &dup_archive)
        .await
        .check_ok();
    let created = boards_of(&ts)
        .await
        .into_iter()
        .find(|b| b.title == "Deduped Board")
        .expect("deduped board imported");
    let members: Vec<serde_json::Value> = ts
        .client
        .get(&format!("{TEST_WS}/containers/{}/members", created.id))
        .await
        .ok_json();
    assert_eq!(
        members
            .iter()
            .filter(|m| m["userId"].as_str() == Some(user2_id.as_str()))
            .count(),
        1,
        "1,000 duplicate member lines produce a single membership: {members:?}"
    );

    // Normal export -> import round-trip.
    let exported = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/archive/export",
            created.id
        ))
        .await;
    exported.check_ok();
    let before_rt = boards_of(&ts).await.len();
    import(&ts.client, TEST_WORKSPACE_ID, &exported.body)
        .await
        .check_ok();
    assert_eq!(
        boards_of(&ts).await.len(),
        before_rt + 1,
        "round-trip imported"
    );

    // A view of a `.fvoci` version 1 archive with a key that no view has is rejected
    // with 400.
    for retired_key in [
        "hiddenOptionIds",
        "visibleOptionIds",
        "collapsedOptionIds",
        "kanbanCalculations",
        "defaultPropertyIds",
        "groupById",
        "sortOptions",
        "datePropertyId",
    ] {
        let mut fields = serde_json::Map::new();
        fields.insert(retired_key.to_owned(), json!([]));
        let bad_jsonl = format!(
            "{}\n{}\n",
            json!({"type": "container", "data": {"id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Legacy View", "createdAt": 1, "updatedAt": 1}}),
            json!({"type": "block", "data": {"id": "v1", "containerId": "b1", "type": "view", "title": "Board", "fields": fields, "createdAt": 1, "updatedAt": 1}})
        );
        let bad_archive = native_archive_entries(&[("b1/container.jsonl", &bad_jsonl)]);
        import(&ts.client, TEST_WORKSPACE_ID, &bad_archive)
            .await
            .check_bad_request();
    }

    ts.teardown().await;
}
