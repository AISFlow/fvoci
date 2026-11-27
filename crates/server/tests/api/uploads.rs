//! Uploads in parts: open, send parts, resume, complete,
//! abandon, and what is refused. The parts kept on the disk are looked at too: nothing
//! of an upload stays behind once it is completed, abandoned or refused.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use futures_util::StreamExt as _;
use fvoci_model::{ContainerMember, ContainerVisibility, UploadPart};
use fvoci_server::app::{CLOSED_UPLOAD_GRACE_MILLIS, UPLOAD_LIFETIME_MILLIS};
use fvoci_server::auth::now_millis;
use fvoci_server::config::{Configuration, S3Config};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TestServer};
use crate::common::{ROOT_WS, TEST_WS};

/// The smallest part size, which the tests use so that a file of two parts stays small.
const PART: usize = 5 * 1024 * 1024;

/// A server whose uploads come in parts of [`PART`] bytes.
async fn server() -> TestServer {
    Setup::native()
        .configure(|config| config.upload_part_bytes = 5 * 1024 * 1024)
        .start()
        .await
        .init_basic()
        .await
}

/// `size` bytes that differ from part to part, so that parts in the wrong order show.
fn file_of(size: usize) -> Vec<u8> {
    (0..size)
        .map(|index| u8::try_from(index % 251).expect("below 251"))
        .collect()
}

/// The `Content-Digest` of `bytes`.
fn digest(bytes: &[u8]) -> String {
    format!("sha-256=:{}:", STANDARD.encode(Sha256::digest(bytes)))
}

async fn open(client: &Client, board_id: &str, name: &str, size: usize) -> ApiResponse {
    client
        .post(
            &format!("{TEST_WS}/containers/{board_id}/uploads"),
            &json!({"name": name, "size": size}),
        )
        .await
}

/// Opens an upload and returns its identifier.
async fn opened(client: &Client, board_id: &str, name: &str, size: usize) -> String {
    let response = open(client, board_id, name, size).await;
    response.check_created();
    response.value()["uploadId"]
        .as_str()
        .expect("an upload has an identifier")
        .to_owned()
}

/// Sends `bytes` as part `number` with the digest `digest`.
async fn put_part_with(
    client: &Client,
    upload_id: &str,
    number: &str,
    bytes: &[u8],
    digest: &str,
) -> ApiResponse {
    let request = client
        .request(
            Method::PUT,
            &format!("{TEST_WS}/uploads/{upload_id}/parts/{number}"),
        )
        .header("Content-Type", "application/octet-stream")
        .header("Content-Digest", digest)
        .body(bytes.to_vec());
    Client::send(request).await
}

/// Sends `bytes` as part `number`, with their digest.
async fn put_part(client: &Client, upload_id: &str, number: usize, bytes: &[u8]) -> ApiResponse {
    put_part_with(
        client,
        upload_id,
        &number.to_string(),
        bytes,
        &digest(bytes),
    )
    .await
}

/// Makes the second client an editor of the board.
async fn second_editor(ts: &TestServer, board_id: &str) {
    let user = ts.client2.me().await;
    ts.client
        .add_member(&ContainerMember {
            container_id: board_id.to_owned(),
            user_id: user.id,
            role: fvoci_model::ContainerRole::Editor,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .check_ok();
}

/// The directory that holds the parts of an upload.
fn parts_dir(ts: &TestServer, upload_id: &str) -> PathBuf {
    ts.app.config().tmp_dir().join("uploads").join(upload_id)
}

/// The numbers of the parts kept in a directory of parts (`<number>.<random>`), sorted.
fn part_numbers(directory: &Path) -> Vec<String> {
    files_in(directory)
        .iter()
        .map(|name| name.split('.').next().unwrap_or_default().to_owned())
        .collect()
}

/// The names of the files in a directory, sorted; none if it is not there.
fn files_in(directory: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|entry| {
            entry
                .expect("an entry is read")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// A file of two parts, sent out of order and once again, completes into the file it
/// was, which downloads whole; the upload and its parts are gone after.
#[tokio::test]
async fn a_file_sent_in_parts_is_stored_whole() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let data = file_of(PART + 1000);

    let response = open(&ts.client, &board.id, "Report.PDF", data.len()).await;
    response.check_created();
    let opened = response.value();
    assert_eq!(opened["partSize"], PART, "the server's part size");
    assert_eq!(opened["partCount"], 2, "a full part and the rest");
    let upload_id = opened["uploadId"]
        .as_str()
        .expect("an identifier")
        .to_owned();
    assert_eq!(upload_id.len(), 43, "256 bits in URL-safe base64");

    put_part(&ts.client, &upload_id, 2, &data[PART..])
        .await
        .check_no_content();
    // What a client that lost its place sees.
    let state = ts
        .client
        .get(&format!("{TEST_WS}/uploads/{upload_id}"))
        .await;
    state.check_ok();
    let state = state.value();
    assert_eq!(state["containerId"], board.id.as_str());
    assert_eq!(state["name"], "Report.PDF");
    assert_eq!(state["size"], data.len());
    assert_eq!(state["parts"], json!([{"number": 2, "size": 1000}]));

    // Part 1 is sent wrong, then right: the second replaces the first.
    let mut wrong = data[..PART].to_vec();
    wrong[0] ^= 1;
    put_part(&ts.client, &upload_id, 1, &wrong)
        .await
        .check_no_content();
    put_part(&ts.client, &upload_id, 1, &data[..PART])
        .await
        .check_no_content();
    assert_eq!(
        part_numbers(&parts_dir(&ts, &upload_id)),
        ["1", "2"],
        "the part that was replaced is gone"
    );

    let response = ts
        .client
        .post_empty(&format!("{TEST_WS}/uploads/{upload_id}/complete"))
        .await;
    response.check_created();
    let file_id = response.value()["fileId"]
        .as_str()
        .expect("a file identifier")
        .to_owned();
    assert_eq!(
        Path::new(&file_id).extension(),
        Some("pdf".as_ref()),
        "the extension is kept, in lower case"
    );
    let download = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{file_id}",
            board.id
        ))
        .await;
    download.check_ok();
    assert!(download.body == data, "the file is the parts in order");

    assert!(
        !parts_dir(&ts, &upload_id).exists(),
        "the parts are gone once the file is made"
    );
    ts.client
        .get(&format!("{TEST_WS}/uploads/{upload_id}"))
        .await
        .check_not_found();
    ts.client
        .post_empty(&format!("{TEST_WS}/uploads/{upload_id}/complete"))
        .await
        .check_not_found();
}

/// An empty file has no part, and completes at once.
#[tokio::test]
async fn an_empty_file_has_no_part() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = open(&ts.client, &board.id, "empty.txt", 0).await;
    response.check_created();
    assert_eq!(response.value()["partCount"], 0);
    let upload_id = response.value()["uploadId"]
        .as_str()
        .expect("an identifier")
        .to_owned();
    put_part(&ts.client, &upload_id, 1, b"")
        .await
        .check_bad_request();
    let response = ts
        .client
        .post_empty(&format!("{TEST_WS}/uploads/{upload_id}/complete"))
        .await;
    response.check_created();
    let file_id = response.value()["fileId"]
        .as_str()
        .expect("an id")
        .to_owned();
    let download = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{file_id}",
            board.id
        ))
        .await;
    download.check_ok();
    assert_eq!(download.body, b"");
}

/// A part of the wrong length, with a wrong or missing digest, or of a number the
/// upload has no part of is 400, and nothing of it is kept on the disk.
#[tokio::test]
async fn a_part_that_fails_a_check_is_not_kept() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let data = file_of(PART + 10);
    let upload_id = opened(&ts.client, &board.id, "a.bin", data.len()).await;
    let last = &data[PART..];

    let refusals = [
        ("2", &last[..9], digest(&last[..9]), "too short"),
        ("1", last, digest(last), "the size of another part"),
        ("2", last, digest(b"something else"), "another digest"),
        ("2", last, "md5=:AAAA:".to_owned(), "no sha-256 digest"),
        ("0", last, digest(last), "no part 0"),
        ("3", last, digest(last), "no part 3"),
        ("two", last, digest(last), "no number"),
    ];
    for (number, bytes, digest, why) in refusals {
        let response = put_part_with(&ts.client, &upload_id, number, bytes, &digest).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{why}");
        assert!(
            files_in(&parts_dir(&ts, &upload_id)).is_empty(),
            "nothing is kept of a part that is {why}"
        );
    }
    let state = ts
        .client
        .get(&format!("{TEST_WS}/uploads/{upload_id}"))
        .await;
    assert_eq!(state.value()["parts"], json!([]), "no part is recorded");

    // A part without a digest header at all.
    let request = ts
        .client
        .request(
            Method::PUT,
            &format!("{TEST_WS}/uploads/{upload_id}/parts/2"),
        )
        .body(last.to_vec());
    let response = Client::send(request).await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "header Content-Digest");

    // Completing with a part missing names it, and the upload stays open for it.
    put_part(&ts.client, &upload_id, 2, last)
        .await
        .check_no_content();
    let response = ts
        .client
        .post_empty(&format!("{TEST_WS}/uploads/{upload_id}/complete"))
        .await;
    response.check_bad_request();
    assert_eq!(response.error_message(), "parts missing: 1");
    put_part(&ts.client, &upload_id, 1, &data[..PART])
        .await
        .check_no_content();
    ts.client
        .post_empty(&format!("{TEST_WS}/uploads/{upload_id}/complete"))
        .await
        .check_created();
}

/// An upload is its user's: to anyone else it is not there, as one that does not exist.
#[tokio::test]
async fn another_users_upload_is_not_found() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let upload_id = opened(&ts.client, &board.id, "a.txt", 3).await;

    for id in [upload_id.as_str(), "no-such-upload"] {
        let other = &ts.client2;
        other
            .get(&format!("{TEST_WS}/uploads/{id}"))
            .await
            .check_not_found();
        put_part(other, id, 1, b"abc").await.check_not_found();
        other
            .post_empty(&format!("{TEST_WS}/uploads/{id}/complete"))
            .await
            .check_not_found();
        other
            .delete(&format!("{TEST_WS}/uploads/{id}"))
            .await
            .check_not_found();
    }
    // Still there for its user.
    ts.client
        .get(&format!("{TEST_WS}/uploads/{upload_id}"))
        .await
        .check_ok();
}

/// An upload for a board that went to the trash is of the board's workspace still: its
/// user ends it there, and under another workspace it is not there.
#[tokio::test]
async fn an_upload_for_a_board_in_the_trash_is_ended_in_its_workspace() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let upload_id = opened(&ts.client, &board.id, "a.txt", 3).await;
    ts.client
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await
        .check_no_content();

    ts.client
        .delete(&format!("{ROOT_WS}/uploads/{upload_id}"))
        .await
        .check_not_found();
    ts.client
        .delete(&format!("{TEST_WS}/uploads/{upload_id}"))
        .await
        .check_no_content();
}

/// An abandoned upload is gone, its parts with it.
#[tokio::test]
async fn an_abandoned_upload_leaves_nothing() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let upload_id = opened(&ts.client, &board.id, "a.txt", 3).await;
    put_part(&ts.client, &upload_id, 1, b"abc")
        .await
        .check_no_content();
    assert_eq!(part_numbers(&parts_dir(&ts, &upload_id)), ["1"]);

    ts.client
        .delete(&format!("{TEST_WS}/uploads/{upload_id}"))
        .await
        .check_no_content();
    assert!(!parts_dir(&ts, &upload_id).exists(), "the parts are gone");
    ts.client
        .get(&format!("{TEST_WS}/uploads/{upload_id}"))
        .await
        .check_not_found();
    ts.client
        .post_empty(&format!("{TEST_WS}/uploads/{upload_id}/complete"))
        .await
        .check_not_found();
}

/// Opening an upload takes the permission of `uploadFile`, a name, and a size within
/// `max_file_bytes`.
#[tokio::test]
async fn opening_an_upload_is_checked() {
    let ts = Setup::native()
        .configure(|config| config.max_file_bytes = 1000)
        .start()
        .await
        .init_basic()
        .await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    open(&ts.client, &board.id, "a.txt", 1000)
        .await
        .check_created();
    let response = open(&ts.client, &board.id, "a.txt", 1001).await;
    response.check_request_entity_too_large();
    let problem = response.value();
    assert_eq!(
        (&problem["type"], &problem["reason"], &problem["limit"]),
        (
            &json!("/problems/file-too-large"),
            &json!("file_too_large"),
            &json!(1000)
        ),
        "{problem}"
    );

    let refusals: [(Value, &str); 11] = [
        (json!({"name": "", "size": 1}), "name"),
        (json!({"name": "dir/", "size": 1}), "name"),
        (json!({"name": r"dir\", "size": 1}), "name"),
        (json!({"name": ".", "size": 1}), "name"),
        (json!({"name": "dir/..", "size": 1}), "name"),
        (json!({"name": "a\u{0}.txt", "size": 1}), "name"),
        (json!({"name": "\u{0}", "size": 1}), "name"),
        (json!({"name": "a".repeat(256), "size": 1}), "name"),
        (json!({"name": "a.txt", "size": -1}), "size"),
        (
            json!({"name": "a.txt", "size": 1, "contentType": "a".repeat(256)}),
            "contentType",
        ),
        (json!({"name": "a.txt", "size": 1, "fileId": "x"}), ""),
    ];
    for (body, field) in refusals {
        let response = ts
            .client
            .post(&format!("{TEST_WS}/containers/{}/uploads", board.id), &body)
            .await;
        response.check_bad_request();
        if !field.is_empty() {
            assert_eq!(response.refused_at(), format!("/{field}"), "{body}");
        }
    }

    // Not a member of the board.
    let response = open(&ts.client2, &board.id, "a.txt", 3).await;
    assert!(
        matches!(
            response.status,
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ),
        "{}",
        response.status
    );
}

/// A user who may no longer upload to the board cannot complete the upload, and it is
/// thrown away.
#[tokio::test]
async fn a_completion_without_the_permission_throws_the_upload_away() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let editor = ts.client2.me().await;
    ts.client
        .add_member(&ContainerMember {
            container_id: board.id.clone(),
            user_id: editor.id.clone(),
            role: fvoci_model::ContainerRole::Editor,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .check_ok();
    let upload_id = opened(&ts.client2, &board.id, "a.txt", 3).await;
    put_part(&ts.client2, &upload_id, 1, b"abc")
        .await
        .check_no_content();

    ts.client
        .delete(&format!(
            "{TEST_WS}/containers/{}/members/{}",
            board.id, editor.id
        ))
        .await
        .check_no_content();
    let response = ts
        .client2
        .post_empty(&format!("{TEST_WS}/uploads/{upload_id}/complete"))
        .await;
    // The board is private, and the user no member of it any more.
    response.check_not_found();
    assert!(!parts_dir(&ts, &upload_id).exists(), "the parts are gone");
    ts.client2
        .get(&format!("{TEST_WS}/uploads/{upload_id}"))
        .await
        .check_not_found();
}

/// An upload that is closed, as it is while it is completed, takes no part, no
/// abandonment and no second completion.
#[tokio::test]
async fn a_closed_upload_is_left_to_whoever_closed_it() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let upload_id = opened(&ts.client, &board.id, "a.txt", 3).await;
    put_part(&ts.client, &upload_id, 1, b"abc")
        .await
        .check_no_content();
    assert!(
        ts.app
            .store()
            .close_upload(&upload_id, now_millis())
            .await
            .expect("the upload is closed")
    );

    let conflicts = [
        put_part(&ts.client, &upload_id, 1, b"abc").await,
        ts.client
            .delete(&format!("{TEST_WS}/uploads/{upload_id}"))
            .await,
        ts.client
            .post_empty(&format!("{TEST_WS}/uploads/{upload_id}/complete"))
            .await,
    ];
    for response in conflicts {
        assert_eq!(response.status, StatusCode::CONFLICT, "{}", response.text());
    }
    assert_eq!(
        part_numbers(&parts_dir(&ts, &upload_id)),
        ["1"],
        "what is there is left to whoever closed the upload"
    );
}

/// A part that is still coming in when the upload is closed is not kept: it would
/// otherwise stay behind once the upload is gone.
#[tokio::test]
async fn a_part_that_ends_after_the_upload_closed_is_not_kept() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let upload_id = opened(&ts.client, &board.id, "a.txt", 3).await;
    let me = ts.client.me().await;
    let (upload, _) = ts
        .app
        .upload(&upload_id, &me.id)
        .await
        .expect("the upload is there");

    // The first byte comes in, then the upload is closed, then the rest comes.
    let app = &ts.app;
    let id = upload.id.as_str();
    let chunks = futures_util::stream::iter([&b"a"[..], &b"bc"[..]])
        .enumerate()
        .then(|(index, chunk)| async move {
            if index == 1 {
                let closed = app.store().close_upload(id, now_millis()).await;
                assert_eq!(closed.ok(), Some(true), "the upload is closed in between");
            }
            Ok::<_, std::io::Error>(Bytes::from_static(chunk))
        });
    let digest: [u8; 32] = Sha256::digest(b"abc").into();
    let refused = ts
        .app
        .put_upload_part(&upload, 1, 3, &digest, Box::pin(chunks))
        .await
        .expect_err("the part is refused");
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert!(
        !parts_dir(&ts, &upload_id).exists(),
        "nothing is left of the part: {:?}",
        files_in(&parts_dir(&ts, &upload_id))
    );
}

/// A part sent again that ends after the upload was closed leaves the part that was
/// recorded as it was: a completion that closed the upload counts on it.
#[tokio::test]
async fn a_late_part_sent_again_leaves_the_recorded_part() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let upload_id = opened(&ts.client, &board.id, "a.txt", 3).await;
    put_part(&ts.client, &upload_id, 1, b"abc")
        .await
        .check_no_content();
    let me = ts.client.me().await;
    let (upload, parts) = ts
        .app
        .upload(&upload_id, &me.id)
        .await
        .expect("the upload is there");
    let recorded = parts_dir(&ts, &upload_id).join(&parts[0].part_id);

    let app = &ts.app;
    let id = upload.id.as_str();
    let chunks = futures_util::stream::iter([&b"x"[..], &b"yz"[..]])
        .enumerate()
        .then(|(index, chunk)| async move {
            if index == 1 {
                let closed = app.store().close_upload(id, now_millis()).await;
                assert_eq!(closed.ok(), Some(true), "the upload is closed in between");
            }
            Ok::<_, std::io::Error>(Bytes::from_static(chunk))
        });
    let digest: [u8; 32] = Sha256::digest(b"xyz").into();
    let refused = ts
        .app
        .put_upload_part(&upload, 1, 3, &digest, Box::pin(chunks))
        .await
        .expect_err("the part is refused");
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(
        std::fs::read(&recorded).expect("the recorded part is there"),
        b"abc",
        "the recorded part is as it was"
    );
    assert_eq!(part_numbers(&parts_dir(&ts, &upload_id)), ["1"]);
}

/// A user has at most eight uploads open; one that is abandoned, or closed, makes room.
#[tokio::test]
async fn a_user_has_at_most_eight_uploads_open() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let mut open_ones = Vec::new();
    for _ in 0..8 {
        open_ones.push(opened(&ts.client, &board.id, "a.txt", 1).await);
    }
    let response = open(&ts.client, &board.id, "a.txt", 1).await;
    response.check_status(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.error_reason(), "too_many_uploads");
    second_editor(&ts, &board.id).await;
    // Another user is not held back by this one.
    open(&ts.client2, &board.id, "a.txt", 1)
        .await
        .check_created();

    ts.client
        .delete(&format!("{TEST_WS}/uploads/{}", open_ones[0]))
        .await
        .check_no_content();
    open(&ts.client, &board.id, "a.txt", 1)
        .await
        .check_created();
    // A session that is closed and not thrown away yet, as after a crash, holds nothing.
    ts.app
        .store()
        .close_upload(&open_ones[1], now_millis())
        .await
        .expect("the upload is closed");
    open(&ts.client, &board.id, "a.txt", 1)
        .await
        .check_created();
    open(&ts.client, &board.id, "a.txt", 1)
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);
}

/// The open uploads of a user hold back at most twice the largest file, and those of
/// the server at most `max_pending_upload_bytes`.
#[tokio::test]
async fn the_bytes_open_uploads_hold_back_are_limited() {
    let ts = Setup::native()
        .configure(|config| {
            config.max_file_bytes = 1000;
            config.max_import_bytes = 1000;
            config.max_pending_upload_bytes = 2500;
        })
        .start()
        .await
        .init_basic()
        .await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    opened(&ts.client, &board.id, "a.bin", 1000).await;
    opened(&ts.client, &board.id, "a.bin", 1000).await;
    let response = open(&ts.client, &board.id, "a.bin", 1).await;
    response.check_status(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.error_reason(), "uploads_pending");

    second_editor(&ts, &board.id).await;
    opened(&ts.client2, &board.id, "a.bin", 500).await;
    let response = open(&ts.client2, &board.id, "a.bin", 1).await;
    response.check_status(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.error_reason(), "server_uploads_pending");
}

/// The clean-up throws away the uploads that have ended, and those left closed for an
/// hour, with their parts; the others stay.
#[tokio::test]
async fn ended_uploads_are_thrown_away_with_their_parts() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let ending = opened(&ts.client, &board.id, "a.txt", 3).await;
    put_part(&ts.client, &ending, 1, b"abc")
        .await
        .check_no_content();
    let now = now_millis();
    assert_eq!(
        ts.app
            .clean_up_uploads(now)
            .await
            .expect("the clean-up runs")
            .removed,
        0,
        "nothing has ended"
    );
    assert_eq!(
        ts.app
            .clean_up_uploads(now + UPLOAD_LIFETIME_MILLIS + 1)
            .await
            .expect("the clean-up runs")
            .removed,
        1
    );
    assert!(!parts_dir(&ts, &ending).exists(), "the parts are gone");
    ts.client
        .get(&format!("{TEST_WS}/uploads/{ending}"))
        .await
        .check_not_found();

    let left_closed = opened(&ts.client, &board.id, "a.txt", 3).await;
    put_part(&ts.client, &left_closed, 1, b"abc")
        .await
        .check_no_content();
    ts.app
        .store()
        .close_upload(&left_closed, now)
        .await
        .expect("the upload is closed");
    assert_eq!(
        ts.app
            .clean_up_uploads(now + 1000)
            .await
            .expect("runs")
            .removed,
        0,
        "a closed upload is left to whoever closed it for a while"
    );
    assert_eq!(
        ts.app
            .clean_up_uploads(now + CLOSED_UPLOAD_GRACE_MILLIS + 1)
            .await
            .expect("runs")
            .removed,
        1
    );
    assert!(!parts_dir(&ts, &left_closed).exists(), "the parts are gone");
}

/// The sweep of the start removes what no session owns: stray directories, those a
/// completion moved aside, and the files the local storage was writing.
#[tokio::test]
async fn the_sweep_removes_what_no_upload_owns() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let live = opened(&ts.client, &board.id, "a.txt", 3).await;
    put_part(&ts.client, &live, 1, b"abc")
        .await
        .check_no_content();
    let uploads = ts.app.config().tmp_dir().join("uploads");
    let writing = Path::new(&ts.app.config().file_storage_path).join(".tmp/writing");
    let strays = [
        uploads.join("gone-upload").join("1.abc"),
        uploads.join(format!("{live}.completing-xyz")).join("1.abc"),
        writing.join("half-written"),
    ];
    for stray in &strays {
        std::fs::create_dir_all(stray.parent().expect("a parent")).expect("made");
        std::fs::write(stray, b"left over").expect("written");
    }

    let removed = ts
        .app
        .sweep_temporary_files()
        .await
        .expect("the sweep runs");
    assert_eq!(removed, 3);
    for stray in &strays {
        assert!(!stray.exists(), "{} is removed", stray.display());
    }
    assert_eq!(
        files_in(&uploads),
        std::slice::from_ref(&live),
        "the live upload's parts stay"
    );
    assert_eq!(part_numbers(&parts_dir(&ts, &live)), ["1"]);
}

/// `uploadFile` takes one part's worth; a larger file is refused with the reason to send
/// it in parts, whether the request declares it or not, and nothing of it is kept.
#[tokio::test]
async fn a_single_upload_takes_one_part() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let files = Path::new(&ts.app.config().file_storage_path).to_path_buf();

    let response = crate::files::upload(&ts.client, &board.id, "a.bin", &file_of(PART)).await;
    response.check_created();
    let stored = stored_files(&files);
    assert_eq!(stored.len(), 1);

    // Within the allowance for the form, so the file is cut off as it is read; and far
    // over it, so the request is refused for what it declares.
    for size in [PART + 1, PART + 1024 * 1024] {
        let response = crate::files::upload(&ts.client, &board.id, "a.bin", &file_of(size)).await;
        response.check_request_entity_too_large();
        assert_eq!(response.error_reason(), "upload_in_parts", "{size} bytes");
        assert_eq!(
            stored_files(&files),
            stored,
            "nothing of {size} bytes is kept"
        );
        assert_eq!(
            files_in(&files.join(".tmp/writing")),
            Vec::<String>::new(),
            "no partial file of {size} bytes is left"
        );
    }
}

/// Every file under the files directory but the temporary ones, by path.
fn stored_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("an entry is read").path();
            if path.file_name().is_some_and(|name| name == ".tmp") {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Uploads opened at the same moment do not pass the limit together.
#[tokio::test]
async fn uploads_opened_at_once_keep_to_the_limit() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let answers =
        futures_util::future::join_all((0..20).map(|_| open(&ts.client, &board.id, "a.txt", 1)))
            .await;
    let opened = answers
        .iter()
        .filter(|answer| answer.status == StatusCode::CREATED)
        .count();
    let refused = answers
        .iter()
        .filter(|answer| answer.status == StatusCode::TOO_MANY_REQUESTS)
        .count();
    assert_eq!((opened, refused), (8, 12));
}

/// An upload closed just before its end, as by a completion, is not thrown away under
/// whoever closed it when it ends: it gets the grace of a closed upload.
#[tokio::test]
async fn an_upload_closed_just_before_its_end_gets_the_grace() {
    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = open(&ts.client, &board.id, "a.txt", 3).await;
    response.check_created();
    let expires_at = response.value()["expiresAt"].as_i64().expect("an end");
    let upload_id = response.value()["uploadId"]
        .as_str()
        .expect("an identifier")
        .to_owned();
    put_part(&ts.client, &upload_id, 1, b"abc")
        .await
        .check_no_content();
    ts.app
        .store()
        .close_upload(&upload_id, expires_at - 1000)
        .await
        .expect("the upload is closed");

    let removed = ts
        .app
        .clean_up_uploads(expires_at + 30_000)
        .await
        .expect("the clean-up runs")
        .removed;
    assert_eq!(removed, 0, "the completion is left alone");
    assert_eq!(part_numbers(&parts_dir(&ts, &upload_id)), ["1"]);
    let removed = ts
        .app
        .clean_up_uploads(expires_at - 1000 + CLOSED_UPLOAD_GRACE_MILLIS + 1)
        .await
        .expect("the clean-up runs")
        .removed;
    assert_eq!(removed, 1, "after the grace it is thrown away");
    assert!(!parts_dir(&ts, &upload_id).exists());
}

/// A client that waits for `100 Continue` before it sends a file that is declared too
/// large is answered at once, and is not invited to send it.
#[tokio::test]
async fn a_single_upload_declared_too_large_is_refused_before_it_is_sent() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let ts = server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let address = ts.base_url.trim_start_matches("http://").to_owned();
    let request = format!(
        "POST /api/v1/w/{TEST_WORKSPACE_ID}/containers/{}/files HTTP/1.1\r\nHost: {address}\r\n\
         Authorization: Bearer {}\r\nX-Requested-With: XMLHttpRequest\r\n\
         Content-Type: multipart/form-data; boundary=edge\r\n\
         Content-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
        board.id,
        ts.client.token,
        PART * 4
    );
    let mut stream = tokio::net::TcpStream::connect(&address)
        .await
        .expect("the server accepts the connection");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("the headers are sent");

    // Nothing of the body is sent: the answer comes, or the test runs out of time.
    let mut answer = Vec::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        stream.read_to_end(&mut answer),
    )
    .await;
    assert!(read.is_ok(), "the answer came without the body");
    let answer = String::from_utf8_lossy(&answer);
    assert!(
        answer.starts_with("HTTP/1.1 413"),
        "refused, not asked to continue: {answer}"
    );
    assert!(answer.contains("upload_in_parts"), "{answer}");
}

/// Files go to the S3-compatible server of `FVOCI_S3_TEST_ENDPOINT`, into `bucket`, as
/// the storage tests use it (`tests/storage.rs`).
pub(crate) fn configure_s3(config: &mut Configuration, bucket: &str) {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let or = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
    config.upload_part_bytes = 5 * 1024 * 1024;
    "s3".clone_into(&mut config.file_storage);
    config.file_storage_s3 = S3Config {
        access_key_id: or("FVOCI_S3_TEST_ACCESS_KEY", "any"),
        secret_access_key: or("FVOCI_S3_TEST_SECRET_KEY", "any"),
        bucket: bucket.to_owned(),
        key_prefix: "upload-tests".to_owned(),
        region: or("FVOCI_S3_TEST_REGION", "us-east-1"),
        endpoint: var("FVOCI_S3_TEST_ENDPOINT"),
        https: var("FVOCI_S3_TEST_SSL") == "true",
        ..S3Config::default()
    };
}

/// A server whose files go to the bucket of `FVOCI_S3_TEST_BUCKET`.
pub(crate) async fn s3_server() -> TestServer {
    assert!(
        std::env::var("FVOCI_S3_TEST_ENDPOINT").is_ok()
            && std::env::var("FVOCI_S3_TEST_BUCKET").is_ok(),
        "set FVOCI_S3_TEST_ENDPOINT and FVOCI_S3_TEST_BUCKET to run this test"
    );
    Setup::native()
        .configure(|config| {
            let bucket = std::env::var("FVOCI_S3_TEST_BUCKET").unwrap_or_default();
            configure_s3(config, &bucket);
        })
        .start()
        .await
        .init_basic()
        .await
}

/// The place in the file storage an open upload makes its file at.
async fn upload_path(ts: &TestServer, upload_id: &str) -> String {
    ts.app
        .store()
        .uploads_to_clean_up(i64::MAX, 0)
        .await
        .expect("the uploads are read")
        .into_iter()
        .find(|upload| upload.id == upload_id)
        .expect("the upload is open")
        .path
}

/// With S3, the parts go to the bucket as a multipart upload, not to the disk: a file
/// sent out of order and once again completes into the file it was, an empty file
/// completes too, and an abandoned upload takes no more parts.
#[tokio::test]
#[ignore = "needs an S3-compatible server; see tests/storage.rs"]
async fn with_s3_the_parts_go_to_the_bucket() {
    let ts = s3_server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let data = file_of(PART + 1000);

    let upload_id = opened(&ts.client, &board.id, "Report.PDF", data.len()).await;
    put_part(&ts.client, &upload_id, 2, &data[PART..])
        .await
        .check_no_content();
    let mut wrong = data[..PART].to_vec();
    wrong[0] ^= 1;
    put_part(&ts.client, &upload_id, 1, &wrong)
        .await
        .check_no_content();
    put_part(&ts.client, &upload_id, 1, &data[..PART])
        .await
        .check_no_content();
    assert_eq!(
        files_in(&parts_dir(&ts, &upload_id)),
        Vec::<String>::new(),
        "nothing of the parts is left on the disk once sent"
    );
    let state = ts
        .client
        .get(&format!("{TEST_WS}/uploads/{upload_id}"))
        .await;
    state.check_ok();
    assert_eq!(
        state.value()["parts"],
        json!([{"number": 1, "size": PART}, {"number": 2, "size": 1000}])
    );
    // A part that fails its check does not reach the bucket, nor replace what did.
    let response = put_part_with(&ts.client, &upload_id, "2", &data[PART..], &digest(b"x")).await;
    response.check_bad_request();

    let response = ts
        .client
        .post_empty(&format!("{TEST_WS}/uploads/{upload_id}/complete"))
        .await;
    response.check_created();
    let file_id = response.value()["fileId"]
        .as_str()
        .expect("a file identifier")
        .to_owned();
    assert_eq!(Path::new(&file_id).extension(), Some("pdf".as_ref()));
    let download = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{file_id}",
            board.id
        ))
        .await;
    download.check_ok();
    assert!(download.body == data, "the file is the parts in order");
    ts.client
        .get(&format!("{TEST_WS}/uploads/{upload_id}"))
        .await
        .check_not_found();

    // An empty file has no part to send to the bucket, and completes at once.
    let empty = opened(&ts.client, &board.id, "empty.txt", 0).await;
    let response = ts
        .client
        .post_empty(&format!("{TEST_WS}/uploads/{empty}/complete"))
        .await;
    response.check_created();
    let file_id = response.value()["fileId"]
        .as_str()
        .expect("a file identifier")
        .to_owned();
    let download = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{file_id}",
            board.id
        ))
        .await;
    download.check_ok();
    assert_eq!(download.body.len(), 0, "an empty file");

    // An abandoned upload is aborted in the bucket; what comes after is not found.
    let abandoned = opened(&ts.client, &board.id, "a.txt", 3).await;
    put_part(&ts.client, &abandoned, 1, b"abc")
        .await
        .check_no_content();
    ts.client
        .delete(&format!("{TEST_WS}/uploads/{abandoned}"))
        .await
        .check_no_content();
    put_part(&ts.client, &abandoned, 1, b"abc")
        .await
        .check_not_found();
    ts.client
        .post_empty(&format!("{TEST_WS}/uploads/{abandoned}/complete"))
        .await
        .check_not_found();
}

/// With S3, an upload the bucket does not start leaves no session, a part sent to a
/// closed session does not reach the bucket, and a completion the bucket refuses leaves
/// neither a session nor an object.
#[tokio::test]
#[ignore = "needs an S3-compatible server; see tests/storage.rs"]
async fn with_s3_what_fails_leaves_nothing() {
    let ts = s3_server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    // A part sent after the session was closed, as by a completion that began.
    let upload_id = opened(&ts.client, &board.id, "a.txt", 3).await;
    ts.app
        .store()
        .close_upload(&upload_id, now_millis())
        .await
        .expect("the upload is closed");
    let refused = put_part(&ts.client, &upload_id, 1, b"abc").await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text());
    let parts = ts
        .app
        .store()
        .get_upload_parts(&upload_id)
        .await
        .expect("the parts are read");
    assert_eq!(parts.len(), 0, "nothing is recorded");

    // The bucket refuses the completion: the record of a part is not the bucket's.
    let upload_id = opened(&ts.client, &board.id, "b.txt", 3).await;
    put_part(&ts.client, &upload_id, 1, b"abc")
        .await
        .check_no_content();
    let path = upload_path(&ts, &upload_id).await;
    ts.app
        .store()
        .put_upload_part(
            &upload_id,
            &UploadPart {
                number: 1,
                size: 3,
                part_id: "\"not-the-bucket's\"".to_owned(),
            },
        )
        .await
        .expect("the part is recorded");
    let response = ts
        .client
        .post_empty(&format!("{TEST_WS}/uploads/{upload_id}/complete"))
        .await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    ts.client
        .get(&format!("{TEST_WS}/uploads/{upload_id}"))
        .await
        .check_not_found();
    assert_eq!(
        ts.app.files().size(&path).await.expect("the size is read"),
        None,
        "no object is left"
    );

    // A bucket that is not there starts no upload.
    let ts = Setup::native()
        .configure(|config| configure_s3(config, "fvoci-no-such-bucket"))
        .start()
        .await
        .init_basic()
        .await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = open(&ts.client, &board.id, "a.txt", 3).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    let sessions = ts
        .app
        .store()
        .upload_ids()
        .await
        .expect("the uploads are read");
    assert_eq!(sessions.len(), 0, "no session is left");
}

/// Completes the multipart upload of a session in the bucket behind the server's back,
/// as a completion that took effect with its answer lost, and returns where the object
/// is.
async fn complete_behind_the_back(ts: &TestServer, upload_id: &str) -> String {
    let upload = ts
        .app
        .store()
        .uploads_to_clean_up(i64::MAX, 0)
        .await
        .expect("the uploads are read")
        .into_iter()
        .find(|upload| upload.id == upload_id)
        .expect("the upload is open");
    let parts = ts
        .app
        .store()
        .get_upload_parts(upload_id)
        .await
        .expect("the parts are read");
    ts.app
        .files()
        .multipart()
        .expect("the files are in S3")
        .complete_multipart(
            &upload.path,
            &upload.multipart_id,
            parts.into_iter().map(|part| part.part_id).collect(),
        )
        .await
        .expect("the bucket completes the upload");
    assert_eq!(
        ts.app
            .files()
            .size(&upload.path)
            .await
            .expect("the size is read"),
        Some(3),
        "the object is there"
    );
    upload.path
}

/// With S3, a completion that took effect in the bucket without the file being recorded
/// leaves no object without record: not when the answer was lost and the client
/// completes again, and not when the server stopped before it recorded the file and the
/// clean-up found the session.
#[tokio::test]
#[ignore = "needs an S3-compatible server; see tests/storage.rs"]
async fn with_s3_a_completion_without_record_leaves_no_object() {
    let ts = s3_server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    let upload_id = opened(&ts.client, &board.id, "a.txt", 3).await;
    put_part(&ts.client, &upload_id, 1, b"abc")
        .await
        .check_no_content();
    let path = complete_behind_the_back(&ts, &upload_id).await;
    // Object stores differ: some complete an upload again, some say it is gone. The
    // file is either made and recorded, or not left at all.
    let response = ts
        .client
        .post_empty(&format!("{TEST_WS}/uploads/{upload_id}/complete"))
        .await;
    if response.status == StatusCode::CREATED {
        let file_id = response.value()["fileId"]
            .as_str()
            .expect("a file identifier")
            .to_owned();
        // The key ends in the secret the stored name begins with, before its extension.
        assert_eq!(
            path.rsplit('/').next(),
            file_id.strip_suffix(".txt"),
            "the object is the file"
        );
        let download = ts
            .client
            .get(&format!(
                "{TEST_WS}/containers/{}/files/{file_id}",
                board.id
            ))
            .await;
        download.check_ok();
        assert!(download.body == b"abc".as_slice(), "the file is recorded");
    } else {
        assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            ts.app.files().size(&path).await.expect("the size is read"),
            None,
            "the object without record is gone"
        );
    }

    let upload_id = opened(&ts.client, &board.id, "b.txt", 3).await;
    put_part(&ts.client, &upload_id, 1, b"abc")
        .await
        .check_no_content();
    let path = complete_behind_the_back(&ts, &upload_id).await;
    ts.app
        .store()
        .close_upload(&upload_id, now_millis())
        .await
        .expect("the upload is closed");
    let removed = ts
        .app
        .clean_up_uploads(now_millis() + CLOSED_UPLOAD_GRACE_MILLIS + 1000)
        .await
        .expect("the clean-up runs")
        .removed;
    assert_eq!(removed, 1);
    assert_eq!(
        ts.app.files().size(&path).await.expect("the size is read"),
        None,
        "the object without record is gone"
    );
}

/// With S3, the parts of one upload sent all at once are each taken, and complete into
/// the file.
#[tokio::test]
#[ignore = "needs an S3-compatible server; see tests/storage.rs"]
async fn with_s3_parts_sent_at_once_are_all_taken() {
    let ts = s3_server().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let data = file_of(PART * 3 + 10);
    let upload_id = opened(&ts.client, &board.id, "a.bin", data.len()).await;

    // Distinct parts sent all at once all succeed; duplicate concurrent streams for the
    // same `(upload_id, part_number)` either serialize sequentially (`204`) or are
    // refused with `409 Conflict` while the in-flight part finishes.
    let distinct = (1..=4).map(|number| {
        let start = (number - 1) * PART;
        let end = (start + PART).min(data.len());
        put_part(&ts.client, &upload_id, number, &data[start..end])
    });
    for response in futures_util::future::join_all(distinct).await {
        response.check_no_content();
    }

    let duplicates = (0..8).map(|send| {
        let number = send % 4 + 1;
        let start = (number - 1) * PART;
        let end = (start + PART).min(data.len());
        put_part(&ts.client, &upload_id, number, &data[start..end])
    });
    let outcomes = futures_util::future::join_all(duplicates).await;
    for part_idx in 0..4 {
        let pair = [&outcomes[part_idx], &outcomes[part_idx + 4]];
        assert!(
            pair.iter().any(|r| r.status == StatusCode::NO_CONTENT),
            "at least one stream for part {} must succeed",
            part_idx + 1
        );
        for response in pair {
            assert!(
                matches!(
                    response.status,
                    StatusCode::NO_CONTENT | StatusCode::CONFLICT
                ),
                "unexpected status for concurrent part {}: {} {}",
                part_idx + 1,
                response.status,
                response.text()
            );
        }
    }

    let response = ts
        .client
        .post_empty(&format!("{TEST_WS}/uploads/{upload_id}/complete"))
        .await;
    response.check_created();
    let file_id = response.value()["fileId"]
        .as_str()
        .expect("a file identifier")
        .to_owned();
    let download = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{file_id}",
            board.id
        ))
        .await;
    download.check_ok();
    assert!(download.body == data, "the file is the parts in order");
    assert!(!parts_dir(&ts, &upload_id).exists(), "the parts are gone");
}
