//! File upload, download and info.
use fvoci_model::{Block, BlockType, Container, ContainerVisibility, JsonMap, new_id};
use reqwest::Method;
use reqwest::StatusCode;
use reqwest::multipart::{Form, Part};
use serde_json::{Value, json};

use crate::common::TEST_WS;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TestServer};

/// Uploads `data` under `filename` to a board of the test workspace as the given client.
pub(crate) async fn upload(
    client: &Client,
    board_id: &str,
    filename: &str,
    data: &[u8],
) -> crate::common::ApiResponse {
    upload_in(client, TEST_WORKSPACE_ID, board_id, filename, data).await
}

/// Uploads `data` under `filename` to a board of a workspace as the given client.
pub(crate) async fn upload_in(
    client: &Client,
    workspace_id: &str,
    board_id: &str,
    filename: &str,
    data: &[u8],
) -> crate::common::ApiResponse {
    let part = Part::bytes(data.to_vec())
        .file_name(filename.to_owned())
        .mime_str("application/octet-stream")
        .expect("a valid mime");
    let form = Form::new().part("file", part);
    let request = client
        .request(
            Method::POST,
            &format!("/w/{workspace_id}/containers/{board_id}/files"),
        )
        .multipart(form);
    Client::send(request).await
}

/// A key of the server's making for a file of `board_id` ([`fvoci_model::FileKey`]),
/// under the organization of the board, whichever it is.
pub(crate) async fn key_of(ts: &TestServer, board_id: &str, secret: &str) -> String {
    let org = ts
        .app
        .store()
        .system()
        .org_of_container(board_id)
        .await
        .expect("the board's organization");
    format!("{org}/{board_id}/{secret}")
}

/// The `fileId` of a successful upload.
pub(crate) fn uploaded_id(response: &crate::common::ApiResponse) -> String {
    response.value()["fileId"]
        .as_str()
        .expect("the upload answers a fileId")
        .to_owned()
}

/// Makes an open board owned by the first client with one attachment block that refers
/// to an uploaded file, and returns the board and the file identifier.
async fn board_with_attachment(ts: &TestServer) -> (Container, String) {
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let file_id =
        uploaded_id(&upload(&ts.client, &board.id, "pic.png", b"hello world, a file\n").await);
    let card_id = new_id();
    let mut image_fields = JsonMap::new();
    image_fields.insert("fileId".to_owned(), Value::String(file_id.clone()));
    let blocks = vec![Block {
        id: card_id.clone(),
        parent_id: String::new(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    }];
    // The server gives the card an identifier of its own.
    let created = crate::blocks::insert(&ts.client, &board.id, &blocks).await;
    let card_id = created.into_iter().next().expect("the card is made").id;
    let file_block = [Block {
        id: new_id(),
        parent_id: card_id,
        container_id: board.id.clone(),
        fields: image_fields,
        created_at: 1,
        updated_at: 1,
        block_type: BlockType::Attachment,
        ..Block::default()
    }];
    ts.client
        .insert_blocks(&board.id, &file_block)
        .await
        .check_created();
    (board, file_id)
}

// -- upload ----------------------------------------------------------------------------

/// An unauthenticated user cannot upload a file to a board.
#[tokio::test]
async fn upload_file_a_non_authenticated_user_should_be_rejected() {
    let ts = Setup::native().start().await.init_basic().await;
    let response = upload(&ts.anonymous(), "test-board-id", "test.txt", b"test").await;
    response.check_unauthorized();
    ts.teardown().await;
}

/// Uploading to a missing board answers 404, matching a private board the user cannot see.
#[tokio::test]
async fn upload_file_to_a_board_that_does_not_exist_is_not_found() {
    let ts = Setup::native().start().await.init_basic().await;
    let response = upload(&ts.client, "not-valid-board", "test.txt", b"test").await;
    response.check_error(StatusCode::NOT_FOUND, "board not found");
    ts.teardown().await;
}

/// A member with edit permission can upload a file to the board.
#[tokio::test]
async fn upload_file_with_permissions() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = upload(&ts.client, &board.id, "test.txt", b"test").await;
    response.check_created();
    assert!(!uploaded_id(&response).is_empty(), "the file has an id");
    ts.teardown().await;
}

/// Uploading a file larger than the configured maximum size is rejected.
#[tokio::test]
async fn upload_file_reaching_the_max_file_size() {
    let ts = Setup::native()
        .configure(|config| config.max_file_bytes = 1)
        .start()
        .await
        .init_basic()
        .await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = upload(&ts.client, &board.id, "test.txt", b"test").await;
    response.check_request_entity_too_large();
    // Sending it in parts would not help: the file is over the limit of any upload.
    assert_eq!(response.error_reason(), "file_too_large");
    let problem = response.value();
    assert_eq!(problem["type"], "/problems/file-too-large", "{problem}");
    assert_eq!(problem["limit"], 1, "{problem}");
    ts.teardown().await;
}

/// Uploading without permission: who cannot see a board gets 404; who can view it without
/// edit permission gets 403.
#[tokio::test]
async fn workspace_upload_file_no_permission() {
    let ts = Setup::native().start().await.init_basic().await;
    let board_id = new_id();
    let response = upload(&ts.client, &board_id, "x.bin", &[0_u8; 64]).await;
    response.check_error(StatusCode::NOT_FOUND, "board not found");

    let private = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let response = upload(&ts.client2, &private.id, "x.bin", &[0_u8; 64]).await;
    response.check_error(StatusCode::NOT_FOUND, "board not found");

    let open = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = upload(&ts.client2, &open.id, "x.bin", &[0_u8; 64]).await;
    response.check_forbidden();
    ts.teardown().await;
}

/// A board admin can upload a file to the board.
#[tokio::test]
async fn workspace_upload_file_a_board_admin_can_upload() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = upload(&ts.client, &board.id, "x.bin", &[1_u8; 64]).await;
    response.check_created();
    assert!(!uploaded_id(&response).is_empty(), "the file has an id");
    ts.teardown().await;
}

/// A non-member of an open board is refused when uploading a file to it.
#[tokio::test]
async fn workspace_upload_file_a_non_member_is_forbidden() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = upload(&ts.client2, &board.id, "x.bin", &[1_u8; 64]).await;
    response.check_forbidden();
    ts.teardown().await;
}

/// Uploading a `.jpeg` file stores it with a normalized `.jpg` extension.
#[tokio::test]
async fn upload_maps_jpeg_to_jpg() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let file_id = uploaded_id(&upload(&ts.client, &board.id, "photo.JPEG", b"x").await);
    assert_eq!(
        file_id.rsplit_once('.').map(|(_, extension)| extension),
        Some("jpg"),
        "jpeg is stored as jpg: {file_id}"
    );
    ts.teardown().await;
}

/// The upload answers a generated name: a secret, not an identifier, plus the
/// extension.
#[tokio::test]
async fn upload_answers_a_generated_name() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let file_id = uploaded_id(&upload(&ts.client, &board.id, "picture.png", b"x").await);
    let (stem, extension) = file_id.split_once('.').expect("name has an extension");
    assert_eq!(
        stem.len(),
        fvoci_server::auth::secret::SECRET_LENGTH,
        "a secret: {file_id}"
    );
    assert!(!fvoci_model::is_id(stem), "and no identifier: {file_id}");
    assert_eq!(extension, "png", "the extension is kept");
    ts.teardown().await;
}

/// A hostile upload name is reduced to its base name before it reaches storage, so it
/// cannot traverse out of the uploads directory.
#[tokio::test]
async fn upload_name_with_path_separators_is_reduced_to_its_base() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = upload(&ts.client, &board.id, "../../evil.sh", b"x").await;
    response.check_created();
    let file_id = uploaded_id(&response);
    assert_eq!(
        file_id.rsplit_once('.').map(|(_, extension)| extension),
        Some("sh"),
        "the base extension is kept: {file_id}"
    );
    assert!(!file_id.contains('/'), "no separators in the id: {file_id}");
    ts.teardown().await;
}

/// A name that ends in a directory names no file: what would be kept of it is blank,
/// or a directory's own name. One longer than an upload in parts takes is refused as
/// that refuses it.
#[tokio::test]
async fn an_upload_name_that_names_no_file_is_refused() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let too_long = "가".repeat(256);
    for name in [
        "dir/",
        "a/b/ ",
        "/",
        " ",
        r"dir\",
        ".",
        "..",
        "dir/..",
        r"dir\.",
        too_long.as_str(),
    ] {
        let response = upload(&ts.client, &board.id, name, b"x").await;
        response.check_bad_request();
        assert_eq!(response.refused_at(), "parameter file", "{name:?}");
    }
    // A name in a directory is its last segment, as before, and the longest is taken.
    let longest = "가".repeat(255);
    for name in ["dir/a.txt", r"dir\a.txt", "...", longest.as_str()] {
        upload(&ts.client, &board.id, name, b"x")
            .await
            .check_created();
    }
    ts.teardown().await;
}

/// A download is offered under the name its file was uploaded with, also to who reads
/// the board by its public link; the file id is the name for a reader that knows no
/// `filename*`.
#[tokio::test]
async fn a_download_is_offered_under_the_name_of_the_upload() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let file_id = uploaded_id(&upload(&ts.client, &board.id, "docs/분기 보고서.pdf", b"pdf").await);
    let expected = format!(
        "attachment;filename=\"{file_id}\"; \
         filename*=UTF-8''%EB%B6%84%EA%B8%B0%20%EB%B3%B4%EA%B3%A0%EC%84%9C.pdf"
    );
    let own = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{file_id}",
            board.id
        ))
        .await;
    own.check_ok();
    assert_eq!(own.header("content-disposition"), expected);

    let shared = ts
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/public-link", board.id),
            &json!({"enabled": true}),
        )
        .await;
    shared.check_ok();
    let read_token = shared.value()["token"]
        .as_str()
        .expect("a token")
        .to_owned();
    let by_link = ts
        .anonymous()
        .get(&format!(
            "/shared/{}/files/{file_id}?readToken={read_token}",
            board.id
        ))
        .await;
    by_link.check_ok();
    assert_eq!(by_link.header("content-disposition"), expected);
    ts.teardown().await;
}

// -- info ------------------------------------------------------------------------------

/// Requesting file info for an unknown stored name returns 404.
#[tokio::test]
async fn file_record_of_an_unknown_file_is_not_found() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{}/info",
            board.id, "7unknownunknownunknownunk12.png"
        ))
        .await;
    response.check_error(StatusCode::NOT_FOUND, "file not found");
    ts.teardown().await;
}

/// The info JSON says what a client may know about a file, and neither where it is
/// stored nor which board it is filed under.
#[tokio::test]
async fn file_record_has_the_members_of_the_record() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_attachment(&ts).await;
    let response = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{file_id}/info",
            board.id
        ))
        .await;
    let info = response.ok_json::<Value>();
    let mut members: Vec<&str> = info
        .as_object()
        .expect("the info is an object")
        .keys()
        .map(String::as_str)
        .collect();
    members.sort_unstable();
    assert_eq!(
        members,
        ["createdAt", "extension", "id", "name", "size"],
        "{info}"
    );
    assert!(
        fvoci_model::is_id(info["id"].as_str().unwrap_or_default()),
        "the record has an identifier: {info}"
    );
    assert_eq!(
        info["name"],
        json!("pic.png"),
        "the name it was uploaded with"
    );
    assert_eq!(info["extension"], json!(".png"), "the extension");
    ts.teardown().await;
}

// -- download --------------------------------------------------------------------------

/// A plain download returns the expected headers and file bytes.
#[tokio::test]
async fn download_has_the_expected_headers() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_attachment(&ts).await;
    let response = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{file_id}",
            board.id
        ))
        .await;
    response.check_ok();
    assert_eq!(response.body, b"hello world, a file\n", "the content");
    assert_eq!(
        response.header("content-type"),
        "application/octet-stream",
        "the content type"
    );
    assert_eq!(
        response.header("cache-control"),
        "private, no-cache",
        "cache control"
    );
    assert_eq!(
        response.header("x-content-type-options"),
        "nosniff",
        "nosniff"
    );
    assert_eq!(response.header("x-frame-options"), "DENY", "frame options");
    assert_eq!(
        response.header("content-security-policy"),
        "sandbox; frame-ancestors 'none'",
        "a file opened as a document has no scripts and no origin"
    );
    assert_eq!(response.header("accept-ranges"), "bytes", "accept ranges");
    assert_eq!(response.header("content-length"), "20", "content length");
    // Saved under the name it was uploaded with; the file id is for a reader that knows
    // no `filename*`.
    assert_eq!(
        response.header("content-disposition"),
        format!("attachment;filename=\"{file_id}\"; filename*=UTF-8''pic.png"),
        "the disposition names the file"
    );
    assert!(
        !response.header("last-modified").is_empty(),
        "last modified is set"
    );
    ts.teardown().await;
}

/// A `Range: bytes=0-7` download returns 206 Partial Content with the first eight bytes.
#[tokio::test]
async fn download_with_a_range_is_partial_content() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_attachment(&ts).await;
    let request = ts
        .client
        .request(
            Method::GET,
            &format!("{TEST_WS}/containers/{}/files/{file_id}", board.id),
        )
        .header("Range", "bytes=0-7");
    let response = Client::send(request).await;
    response.check_status(reqwest::StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.body, b"hello wo", "the first eight bytes");
    assert_eq!(
        response.header("content-range"),
        "bytes 0-7/20",
        "the content range"
    );
    assert_eq!(response.header("content-length"), "8", "the range length");
    ts.teardown().await;
}

/// A range past the end is a 416 with a `Content-Range` of `bytes */size`, and a
/// problem that names the header.
#[tokio::test]
async fn download_with_a_range_past_the_end_is_416() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_attachment(&ts).await;
    let request = ts
        .client
        .request(
            Method::GET,
            &format!("{TEST_WS}/containers/{}/files/{file_id}", board.id),
        )
        .header("Range", "bytes=1000-2000");
    let response = Client::send(request).await;
    response.check_status(reqwest::StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        response.header("content-range"),
        "bytes */20",
        "the content range"
    );
    assert_eq!(response.problem_type(), "/problems/range-not-satisfiable");
    assert_eq!(response.refused_at(), "header Range");
    assert_eq!(
        response.header("content-disposition"),
        "",
        "an error is no download"
    );

    // What is no range of bytes at all is the same problem, without a length.
    let request = ts
        .client
        .request(
            Method::GET,
            &format!("{TEST_WS}/containers/{}/files/{file_id}", board.id),
        )
        .header("Range", "lines=1-2");
    let response = Client::send(request).await;
    response.check_status(reqwest::StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(response.header("content-range"), "");

    // A condition that does not hold: the file is as new as the answer.
    let request = ts
        .client
        .request(
            Method::GET,
            &format!("{TEST_WS}/containers/{}/files/{file_id}", board.id),
        )
        .header("If-Unmodified-Since", "Sat, 01 Jan 2000 00:00:00 GMT");
    let response = Client::send(request).await;
    response.check_status(reqwest::StatusCode::PRECONDITION_FAILED);
    assert_eq!(response.problem_type(), "/problems/precondition-failed");
    assert_eq!(response.refused_at(), "header If-Unmodified-Since");
    ts.teardown().await;
}

/// A future `If-Modified-Since` header returns 304 Not Modified.
#[tokio::test]
async fn download_if_modified_since_in_the_future_is_304() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_attachment(&ts).await;
    let request = ts
        .client
        .request(
            Method::GET,
            &format!("{TEST_WS}/containers/{}/files/{file_id}", board.id),
        )
        .header("If-Modified-Since", "Wed, 10 Nov 2060 05:00:00 GMT");
    let response = Client::send(request).await;
    response.check_status(reqwest::StatusCode::NOT_MODIFIED);
    ts.teardown().await;
}

/// Downloading an unknown file name returns 404.
#[tokio::test]
async fn download_of_an_unknown_file_is_not_found() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{}",
            board.id, "7unknownunknownunknownunk12.png"
        ))
        .await;
    response.check_error(StatusCode::NOT_FOUND, "file not found");
    ts.teardown().await;
}

// -- access control --------------------------------------------------------------------

/// An anonymous request for a file is refused.
#[tokio::test]
async fn download_without_a_session_is_refused() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_attachment(&ts).await;
    let response = ts
        .anonymous()
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{file_id}",
            board.id
        ))
        .await;
    response.check_unauthorized();
    ts.teardown().await;
}

/// An open board's files are readable by workspace members, while a private board's
/// files are hidden from non-members.
#[tokio::test]
async fn download_by_a_non_member_is_forbidden() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_attachment(&ts).await;
    let path = format!("{TEST_WS}/containers/{}/files/{file_id}", board.id);
    ts.client2.get(&path).await.check_ok();
    ts.client
        .patch(
            &format!("{TEST_WS}/containers/{}", board.id),
            &json!({"visibility": "private"}),
        )
        .await
        .check_ok();
    ts.client2.get(&path).await.check_not_found();
    ts.teardown().await;
}

/// A board's valid read token lets an anonymous client read its files.
#[tokio::test]
async fn download_with_a_valid_read_token() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let (board, file_id) = board_with_attachment(&ts).await;
    let shared = ts
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/public-link", board.id),
            &json!({"enabled": true}),
        )
        .await;
    shared.check_ok();
    let read_token = shared.value()["token"]
        .as_str()
        .expect("sharing answers the read token")
        .to_owned();

    let right = ts
        .anonymous()
        .get(&format!(
            "/shared/{}/files/{file_id}?readToken={read_token}",
            board.id
        ))
        .await;
    right.check_ok();
    let wrong = ts
        .anonymous()
        .get(&format!(
            "/shared/{}/files/{file_id}?readToken=nope",
            board.id
        ))
        .await;
    wrong.check_not_found();
    ts.teardown().await;
}

// -- copying files on duplication ------------------------------------------------------

/// Duplicating a board copies its image file to a new name and the copy is downloadable,
/// including when duplicated into another workspace.
#[tokio::test]
async fn duplicating_a_board_copies_its_files() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_attachment(&ts).await;
    let user_id = ts.client.me().await.id;
    let other = crate::boards::another_workspace(&ts, &user_id).await;

    for (query, workspace) in [
        (String::new(), TEST_WORKSPACE_ID),
        (format!("?toWorkspace={other}"), other.as_str()),
    ] {
        let duplicate = ts
            .client
            .post(
                &format!("{TEST_WS}/containers/{}/duplicate{query}", board.id),
                &json!({}),
            )
            .await;
        let value = duplicate.created_json::<Value>();
        let new_board_id = value["containers"][0]["id"].as_str().expect("a new board");
        let at = format!("/w/{workspace}/containers/{new_board_id}");
        let blocks = ts
            .client
            .get(&format!("/w/{workspace}/boards/{new_board_id}/blocks"))
            .await
            .ok_json::<Vec<Block>>();
        let image = blocks
            .iter()
            .find(|block| block.block_type == BlockType::Attachment)
            .expect("the copy has an attachment block");
        let new_file_id = image.fields.get("fileId").and_then(Value::as_str);
        let new_file_id = new_file_id.expect("a copied file id");
        assert_ne!(new_file_id, file_id, "the copy has a new file id");
        assert_ne!(new_file_id, "", "the copy names a file ({workspace})");

        let download = ts.client.get(&format!("{at}/files/{new_file_id}")).await;
        download.check_ok();
        assert_eq!(
            download.body, b"hello world, a file\n",
            "the copy has the content"
        );
    }
    ts.teardown().await;
}

/// Duplicating a single card copies the files its attachment blocks refer to, whether a
/// block names its file as `fileId` or as `attachmentId` alone.
#[tokio::test]
async fn duplicating_a_card_copies_its_files() {
    let ts = Setup::native().start().await.init_basic().await;
    for key in ["fileId", "attachmentId"] {
        duplicate_a_card_whose_attachment_names_its_file_as(&ts, key).await;
    }
    ts.teardown().await;
}

async fn duplicate_a_card_whose_attachment_names_its_file_as(ts: &TestServer, key: &str) {
    let (board, file_id) = board_with_attachment(ts).await;
    let blocks = ts
        .client
        .get(&format!("{TEST_WS}/boards/{}/blocks", board.id))
        .await
        .ok_json::<Vec<Block>>();
    let id_of = |block_type: BlockType| {
        blocks
            .iter()
            .find(|block| block.block_type == block_type)
            .map(|block| block.id.clone())
            .expect("the board has the block")
    };
    let card_id = id_of(BlockType::Card);
    if key == "attachmentId" {
        let attachment_id = id_of(BlockType::Attachment);
        ts.client
            .patch(
                &format!("{TEST_WS}/boards/{}/blocks/{attachment_id}", board.id),
                &json!({"updatedFields": {"attachmentId": file_id}, "deletedFields": ["fileId"]}),
            )
            .await
            .check_no_content();
    }

    let copies = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/blocks/{card_id}/duplicate", board.id),
            &json!({}),
        )
        .await
        .created_json::<Vec<Block>>();
    let attachment = copies
        .iter()
        .find(|block| block.block_type == BlockType::Attachment)
        .expect("a copied attachment block");
    let new_file_id = attachment
        .fields
        .get("fileId")
        .and_then(Value::as_str)
        .expect("a copied file id");
    assert_ne!(
        new_file_id, file_id,
        "a copy of the attachment's file ({key})"
    );
    let download = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{new_file_id}",
            board.id
        ))
        .await;
    download.check_ok();
}

// -- files are tied to their board --------------------------------------------------------

/// The private board of the first user with an uploaded file, and a private board of
/// the second user who knows that file's name and has put it into an attachment block of
/// their own board.
struct ForeignFile {
    owner_board: Container,
    file_id: String,
    intruder_board: Container,
}

const SECRET: &[u8] = b"the first user's file\n";

async fn foreign_file(ts: &TestServer) -> ForeignFile {
    let owner_board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let file_id = uploaded_id(&upload(&ts.client, &owner_board.id, "secret.png", SECRET).await);

    let board = Container {
        workspace_id: TEST_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Private,
        ..Container::default()
    };
    let intruder_board: Container = ts.client2.create_board(&board).await.created_json();
    let card_id = new_id();
    let named = |key: &str| {
        let mut fields = JsonMap::new();
        fields.insert(key.to_owned(), Value::String(file_id.clone()));
        fields
    };
    let block = |id: String, parent: &str, block_type: BlockType, fields: JsonMap| Block {
        id,
        parent_id: parent.to_owned(),
        container_id: intruder_board.id.clone(),
        block_type,
        fields,
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    let blocks = vec![
        block(card_id.clone(), "", BlockType::Card, JsonMap::new()),
        block(new_id(), &card_id, BlockType::Attachment, named("fileId")),
        block(
            new_id(),
            &card_id,
            BlockType::Attachment,
            named("attachmentId"),
        ),
    ];
    crate::blocks::insert(&ts.client2, &intruder_board.id, &blocks).await;
    ForeignFile {
        owner_board,
        file_id,
        intruder_board,
    }
}

/// Cross-board file isolation: a member of another board cannot fetch a file through
/// their own board's path even if they know its stored name. A file
/// this server stored is given out through its own board only.
#[tokio::test]
async fn a_file_is_not_given_out_through_another_board() {
    let ts = Setup::native().start().await.init_basic().await;
    let fixture = foreign_file(&ts).await;
    let through_own_board = format!(
        "{TEST_WS}/containers/{}/files/{}",
        fixture.intruder_board.id, fixture.file_id
    );

    // Through another board the file is as missing as one that does not exist: the two
    // answers are the same, so nothing is learned about the files of other boards.
    let missing = format!(
        "{TEST_WS}/containers/{}/files/7unknownunknownunknownunk12.png",
        fixture.intruder_board.id
    );
    for route in [&through_own_board, &missing] {
        let download = ts.client2.get(route).await;
        download.check_error(StatusCode::NOT_FOUND, "file not found");
        let info = ts.client2.get(&format!("{route}/info")).await;
        info.check_error(StatusCode::NOT_FOUND, "file not found");
    }

    // The owner cannot be tricked into it either: the file is the other board's.
    ts.client
        .get(&through_own_board)
        .await
        .check_error(StatusCode::NOT_FOUND, "board not found");
    ts.teardown().await;
}

/// The legitimate reads: through the file's own board, by a member and with the read
/// token of the shared board.
#[tokio::test]
async fn a_file_is_given_out_through_its_own_board() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let fixture = foreign_file(&ts).await;
    let path = format!(
        "{TEST_WS}/containers/{}/files/{}",
        fixture.owner_board.id, fixture.file_id
    );

    let download = ts.client.get(&path).await;
    download.check_ok();
    assert_eq!(download.body, SECRET, "the owner reads the file");
    let info = ts.client.get(&format!("{path}/info")).await.value();
    assert_eq!(info["name"], json!("secret.png"), "and its info: {info}");

    // The second user is no member of that private board: for them it does not exist.
    ts.client2
        .get(&path)
        .await
        .check_error(StatusCode::NOT_FOUND, "board not found");

    let read_token = ts.share(&fixture.owner_board.id).await;
    let shared = ts
        .anonymous()
        .get(&format!(
            "/shared/{}/files/{}?readToken={read_token}",
            fixture.owner_board.id, fixture.file_id
        ))
        .await;
    shared.check_ok();
    assert_eq!(
        shared.body, SECRET,
        "a reader of the shared board reads the file"
    );
    ts.teardown().await;
}

/// The same tie holds where the server itself reads a file by name: duplicating a block
/// or a board does not copy another board's file into the duplicate, and an export does
/// not put it into the archive.
#[tokio::test]
async fn a_file_of_another_board_is_neither_copied_nor_exported() {
    let ts = Setup::native().start().await.init_basic().await;
    let fixture = foreign_file(&ts).await;
    let board_id = &fixture.intruder_board.id;

    let duplicate = ts
        .client2
        .post(
            &format!("{TEST_WS}/containers/{board_id}/duplicate"),
            &json!({}),
        )
        .await
        .created_json::<Value>();
    let copy_id = duplicate["containers"][0]["id"]
        .as_str()
        .expect("a copy")
        .to_owned();
    let blocks: Vec<Block> = ts
        .client2
        .get(&format!("{TEST_WS}/boards/{copy_id}/blocks"))
        .await
        .ok_json();
    for block in blocks
        .iter()
        .filter(|block| block.block_type == BlockType::Attachment)
    {
        for key in ["fileId", "attachmentId"] {
            let Some(name) = block.fields.get(key).and_then(Value::as_str) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let response = ts
                .client2
                .get(&format!("{TEST_WS}/containers/{copy_id}/files/{name}"))
                .await;
            assert_ne!(
                response.body, SECRET,
                "the duplicate's {key} {name} does not give the file out"
            );
        }
    }

    let export = ts
        .client2
        .get(&format!("{TEST_WS}/containers/{board_id}/archive/export"))
        .await;
    export.check_ok();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(export.body.clone()))
        .expect("the export is an archive");
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("an entry");
        let mut content = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut content).expect("the entry is read");
        assert_ne!(
            content, SECRET,
            "the entry {index} is not the other board's file"
        );
    }
    ts.teardown().await;
}

/// The private template that the first user made by duplicating a board with an attachment,
/// the name of the template's copy of the image file, and a private board of the second
/// user. The copy is stored in the template layout, `<workspace>/<board>/<name>`, with a
/// record of its own.
struct TemplateFile {
    template: Container,
    file_id: String,
    other_board: Container,
}

async fn template_file(ts: &TestServer) -> TemplateFile {
    let (board, _) = board_with_attachment(ts).await;
    let duplicated: Value = ts
        .client
        .post_empty(&format!(
            "{TEST_WS}/containers/{}/duplicate?asTemplate=true",
            board.id
        ))
        .await
        .created_json();
    let template: Container =
        serde_json::from_value(duplicated["containers"][0].clone()).expect("the template board");
    let blocks: Vec<Block> = ts
        .client
        .get(&format!(
            "{TEST_WS}/boards/{}/blocks?type=attachment",
            template.id
        ))
        .await
        .ok_json();
    let file_id = blocks[0].fields["fileId"]
        .as_str()
        .expect("the attachment block of the template names its file")
        .to_owned();
    assert_ne!(file_id, "", "the file was copied for the template");
    let other_board: Container = ts
        .client2
        .create_board(&Container {
            workspace_id: TEST_WORKSPACE_ID.to_owned(),
            visibility: ContainerVisibility::Private,
            ..Container::default()
        })
        .await
        .created_json();
    TemplateFile {
        template,
        file_id,
        other_board,
    }
}

/// A file stored on a template board is scoped to that template's path like any other
/// file, and cannot be fetched through an unrelated board.
#[tokio::test]
async fn a_template_file_is_not_given_out_through_another_board() {
    let ts = Setup::native().start().await.init_basic().await;
    let fixture = template_file(&ts).await;
    let through_other = format!(
        "{TEST_WS}/containers/{}/files/{}",
        fixture.other_board.id, fixture.file_id
    );

    let download = ts.client2.get(&through_other).await;
    download.check_error(StatusCode::NOT_FOUND, "file not found");
    let info = ts.client2.get(&format!("{through_other}/info")).await;
    info.check_error(StatusCode::NOT_FOUND, "file not found");
    ts.teardown().await;
}

/// The legitimate read of a template's file: through the template, by who may read it.
#[tokio::test]
async fn a_template_file_is_given_out_through_its_template() {
    let ts = Setup::native().start().await.init_basic().await;
    let fixture = template_file(&ts).await;
    let through_template = format!(
        "{TEST_WS}/containers/{}/files/{}",
        fixture.template.id, fixture.file_id
    );

    let download = ts.client.get(&through_template).await;
    download.check_ok();
    assert_eq!(
        download.body, b"hello world, a file\n",
        "the owner of the template reads its file"
    );
    ts.client
        .get(&format!("{through_template}/info"))
        .await
        .check_ok();
    let _ = &fixture.other_board;
    ts.teardown().await;
}

/// A file in the storage that no record ties to a board is given out through no board,
/// whatever its place in the storage says. Before, such a file was served through any
/// board the requester could read. This server stores no file without a record; the
/// test plants one, in the directory of a board.
#[tokio::test]
async fn a_file_without_a_record_is_given_out_through_no_board() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let other = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;

    let secret = fvoci_server::auth::secret::new_secret();
    let name = format!("{secret}.txt");
    let path = key_of(&ts, &board.id, &secret).await;
    ts.app
        .files()
        .write(&path, &mut &b"an old file"[..])
        .await
        .expect("the file is stored");

    for board_id in [&board.id, &other.id] {
        let route = format!("{TEST_WS}/containers/{board_id}/files/{name}");
        let download = ts.client.get(&route).await;
        download.check_error(StatusCode::NOT_FOUND, "file not found");
        let info = ts.client.get(&format!("{route}/info")).await;
        info.check_error(StatusCode::NOT_FOUND, "file not found");
    }
    ts.teardown().await;
}

/// A block with an empty `fileId` does not stop duplication: the failure is logged and
/// the duplicate succeeds with the empty `fileId` preserved.
#[tokio::test]
async fn duplicating_a_board_with_an_empty_file_reference_works() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let card_id = new_id();
    let mut empty = JsonMap::new();
    empty.insert("fileId".to_owned(), Value::String(String::new()));
    let block = |id: String, parent: &str, block_type: BlockType, fields: JsonMap| Block {
        id,
        parent_id: parent.to_owned(),
        container_id: board.id.clone(),
        block_type,
        fields,
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    let blocks = vec![
        block(card_id.clone(), "", BlockType::Card, JsonMap::new()),
        block(new_id(), &card_id, BlockType::Attachment, empty),
        block(new_id(), &card_id, BlockType::Attachment, JsonMap::new()),
    ];
    crate::blocks::insert(&ts.client, &board.id, &blocks).await;

    let duplicate = ts
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/duplicate", board.id),
            &json!({}),
        )
        .await
        .created_json::<Value>();
    let copies = duplicate["blocks"].as_array().expect("the copied blocks");
    assert_eq!(copies.len(), 3, "all three blocks are copied: {duplicate}");
    let image = copies
        .iter()
        .find(|block| block["type"] == json!("attachment"))
        .expect("the attachment block is copied");
    assert_eq!(
        image["fields"]["fileId"],
        json!(""),
        "and still names no file"
    );
    ts.teardown().await;
}

/// Uploading a file with a multi-byte UTF-8 leading character to a template board stores
/// it under a generated name while preserving the upload name in its metadata record.
#[tokio::test]
async fn a_file_whose_name_begins_with_a_wide_character_is_served() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = Container {
        workspace_id: TEST_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Private,
        is_template: true,
        ..Container::default()
    };
    let board: Container = ts.client.create_board(&board).await.created_json();
    let name = "사진.png";
    let file_id = uploaded_id(&upload(&ts.client, &board.id, name, b"wide").await);
    assert_ne!(
        file_id, name,
        "no upload keeps its name, on a template either"
    );
    assert!(file_id.is_ascii(), "the stored name is plain: {file_id}");
    assert_eq!(
        file_id.rsplit_once('.').map(|(_, extension)| extension),
        Some("png"),
        "with the extension: {file_id}"
    );
    let name = file_id.as_str();

    let path = format!("{TEST_WS}/containers/{}/files/{file_id}", board.id);
    let download = ts.client.get(&path).await;
    download.check_ok();
    assert_eq!(download.body, b"wide", "the file is served");
    let info = ts.client.get(&format!("{path}/info")).await.value();
    assert_eq!(
        info["name"],
        json!("사진.png"),
        "the record has the name it was uploaded with: {info}"
    );

    // An attachment block that names it: the board exports and duplicates.
    let mut fields = JsonMap::new();
    fields.insert("fileId".to_owned(), Value::String(name.to_owned()));
    let card_id = new_id();
    let block = |id: String, parent: &str, block_type: BlockType, fields: JsonMap| Block {
        id,
        parent_id: parent.to_owned(),
        container_id: board.id.clone(),
        block_type,
        fields,
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    let blocks = vec![
        block(card_id.clone(), "", BlockType::Card, JsonMap::new()),
        block(new_id(), &card_id, BlockType::Attachment, fields),
    ];
    crate::blocks::insert(&ts.client, &board.id, &blocks).await;
    ts.client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", board.id))
        .await
        .check_ok();
    let duplicate = ts
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/duplicate", board.id),
            &json!({}),
        )
        .await
        .created_json::<Value>();
    let copy_id = duplicate["containers"][0]["id"]
        .as_str()
        .expect("a copy")
        .to_owned();
    let copied: Vec<Block> = ts
        .client
        .get(&format!("{TEST_WS}/boards/{copy_id}/blocks"))
        .await
        .ok_json();
    let image = copied
        .iter()
        .find(|block| block.block_type == BlockType::Attachment)
        .expect("the attachment block is copied");
    let copy_name = image.fields["fileId"].as_str().expect("a file name");
    assert!(!copy_name.is_empty(), "the copy names a file of its own");
    let copy = ts
        .client
        .get(&format!("{TEST_WS}/containers/{copy_id}/files/{copy_name}"))
        .await;
    copy.check_ok();
    assert_eq!(copy.body, b"wide", "with the same content");
    ts.teardown().await;
}

/// A byte range on an empty file returns 200 with the empty body.
#[tokio::test]
async fn a_range_on_an_empty_file_is_ignored() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let file_id = uploaded_id(&upload(&ts.client, &board.id, "empty.txt", b"").await);
    let path = format!("{TEST_WS}/containers/{}/files/{file_id}", board.id);

    for range in ["bytes=-5", "bytes=0-", "bytes=0-10", "bytes=3-"] {
        let request = ts.client.request(Method::GET, &path).header("Range", range);
        let response = Client::send(request).await;
        response.check_ok();
        assert!(response.body.is_empty(), "{range}: the empty file");
    }
    ts.teardown().await;
}

/// Blocks posted with `sourceBoardId` get files of their own on the board they are
/// posted to: the attachment block that is answered and stored names the copy, the copy is
/// read through the target board, and the source keeps its file. (The blocks are not
/// stored when their files are copied; pointing them at the copies must not look them
/// up.)
#[tokio::test]
async fn blocks_posted_from_a_source_board_get_copies_of_their_files() {
    let ts = Setup::native().start().await.init_basic().await;
    let (source, file_id) = board_with_attachment(&ts).await;
    let target = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let mut image_fields = JsonMap::new();
    image_fields.insert("fileId".to_owned(), Value::String(file_id.clone()));
    let target_card = crate::blocks::insert_one(
        &ts.client,
        &Block {
            id: new_id(),
            container_id: target.id.clone(),
            block_type: BlockType::Card,
            created_at: 1,
            updated_at: 1,
            ..Block::default()
        },
    )
    .await;
    let blocks = vec![Block {
        id: new_id(),
        container_id: target.id.clone(),
        parent_id: target_card.id,
        block_type: BlockType::Attachment,
        fields: image_fields,
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    }];

    let response = ts
        .client
        .post(
            &format!(
                "{TEST_WS}/boards/{}/blocks?sourceBoardId={}",
                target.id, source.id
            ),
            &blocks,
        )
        .await;

    let answered: Vec<Block> = response.created_json();
    let image = answered
        .iter()
        .find(|block| block.block_type == BlockType::Attachment)
        .expect("the attachment block is answered");
    let copy_id = image.fields["fileId"]
        .as_str()
        .expect("the image names a file")
        .to_owned();
    assert_ne!(copy_id, file_id, "a file of its own");
    assert!(!copy_id.is_empty(), "and it names one");
    let stored = ts
        .app
        .block(&image.id)
        .await
        .expect("the attachment block is stored");
    assert_eq!(
        stored.fields["fileId"],
        Value::String(copy_id.clone()),
        "what is stored is what was answered"
    );

    let file_path =
        |board: &Container, name: &str| format!("{TEST_WS}/containers/{}/files/{name}", board.id);
    let copy = ts.client.get(&file_path(&target, &copy_id)).await;
    copy.check_ok();
    assert_eq!(
        copy.body, b"hello world, a file\n",
        "the copy has the bytes"
    );
    ts.client
        .get(&file_path(&source, &file_id))
        .await
        .check_ok();
    // Each board has its own: the copy is not the source board's, nor the other way.
    ts.client
        .get(&file_path(&source, &copy_id))
        .await
        .check_not_found();
    ts.client
        .get(&file_path(&target, &file_id))
        .await
        .check_not_found();

    ts.teardown().await;
}
