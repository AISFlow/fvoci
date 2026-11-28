//! The versions kept of a file: listed and read by who reads its board, written from
//! an uploaded file and gone back to by who edits it, each write made from the version
//! it names; kept as many as the server is set to; and gone with the file.

use fvoci_model::{Block, BlockType, Container, ContainerVisibility, FileRecord, JsonMap, new_id};
use fvoci_server::app::ChangeNotification;
use fvoci_server::auth::now_millis;
use fvoci_store::testing::db_type_of_the_tests;
use fvoci_store::{BlockFilter, DbType, VersionAdded};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::files::{upload, uploaded_id};
use crate::loose_objects::{exists, remove_doomed};

fn versions_path(board_id: &str, file_id: &str) -> String {
    format!("{TEST_WS}/containers/{board_id}/files/{file_id}/versions")
}

async fn versions(client: &Client, board_id: &str, file_id: &str) -> Vec<Value> {
    client
        .get(&versions_path(board_id, file_id))
        .await
        .ok_json()
}

/// What a list says of each version: its number, whether it is the newest, and the
/// version it went back to.
fn said(versions: &[Value]) -> Vec<(i64, bool, Option<i64>)> {
    versions
        .iter()
        .map(|version| {
            (
                version["version"].as_i64().expect("a number"),
                version["isCurrent"].as_bool().expect("a flag"),
                version["restoredFrom"].as_i64(),
            )
        })
        .collect()
}

/// Uploads `data` to the board and asks that it become the file's version, made from
/// `base`. Returns the answer and the upload's identifier.
async fn add_version(
    client: &Client,
    board_id: &str,
    file_id: &str,
    base: i64,
    data: &[u8],
) -> (ApiResponse, String) {
    let uploaded = uploaded_id(&upload(client, board_id, "next.txt", data).await);
    let answer = client
        .post(
            &versions_path(board_id, file_id),
            &json!({"baseVersion": base, "fileId": uploaded}),
        )
        .await;
    (answer, uploaded)
}

async fn restore(
    client: &Client,
    board_id: &str,
    file_id: &str,
    version: i64,
    base: i64,
) -> ApiResponse {
    client
        .post(
            &format!("{}/{version}/restore", versions_path(board_id, file_id)),
            &json!({"baseVersion": base}),
        )
        .await
}

async fn bytes_at(client: &Client, path: &str) -> Vec<u8> {
    let response = client.get(path).await;
    response.check_ok();
    response.body
}

async fn file_bytes(client: &Client, board_id: &str, file_id: &str) -> Vec<u8> {
    bytes_at(
        client,
        &format!("{TEST_WS}/containers/{board_id}/files/{file_id}"),
    )
    .await
}

async fn version_bytes(client: &Client, board_id: &str, file_id: &str, version: i64) -> Vec<u8> {
    bytes_at(
        client,
        &format!("{}/{version}", versions_path(board_id, file_id)),
    )
    .await
}

/// An open board of the first user with a file `a.txt` that says "first".
async fn board_with_file(ts: &TestServer) -> (Container, String) {
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let file_id = uploaded_id(&upload(&ts.client, &board.id, "a.txt", b"first").await);
    (board, file_id)
}

/// Attaches the file to a new card of the board: a block shows it from then on.
/// Attaches the file to a new card of the board; the card.
async fn attach(ts: &TestServer, board_id: &str, file_id: &str) -> String {
    let card = Block {
        id: new_id(),
        parent_id: String::new(),
        container_id: board_id.to_owned(),
        block_type: BlockType::Card,
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    let made = crate::blocks::insert(&ts.client, board_id, &[card]).await;
    let mut fields = JsonMap::new();
    fields.insert("fileId".to_owned(), Value::String(file_id.to_owned()));
    let attachment = Block {
        id: new_id(),
        parent_id: made[0].id.clone(),
        container_id: board_id.to_owned(),
        block_type: BlockType::Attachment,
        fields,
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    crate::blocks::insert(&ts.client, board_id, &[attachment]).await;
    made[0].id.clone()
}

/// The size the attachment of a file says, if it says one.
async fn attached_size(ts: &TestServer, board_id: &str, file_id: &str) -> Option<i64> {
    let attachments = BlockFilter {
        block_type: Some(BlockType::Attachment),
        ..BlockFilter::container(board_id)
    };
    let blocks = ts
        .app
        .store()
        .get_blocks(&attachments)
        .await
        .expect("the attachments are read");
    let block = blocks
        .iter()
        .find(|block| block.fields.get("fileId").and_then(Value::as_str) == Some(file_id))
        .expect("the file is attached");
    block.fields.get("size").and_then(Value::as_i64)
}

/// The places in the file storage of the versions kept of a file, the newest first.
async fn places(ts: &TestServer, board_id: &str, file_id: &str) -> Vec<String> {
    ts.app
        .store()
        .file_versions(board_id, file_id)
        .await
        .expect("the versions are read")
        .into_iter()
        .map(|version| version.path)
        .collect()
}

#[track_caller]
fn check_stale(answer: &ApiResponse) {
    answer.check_status(StatusCode::CONFLICT);
    assert_eq!(
        answer.error_reason(),
        "file_version_stale",
        "the reason of the conflict"
    );
    assert_eq!(
        answer.problem_type(),
        "/problems/file-version-stale",
        "the problem's page"
    );
}

/// A file that was never written anew has one version, its upload, by who uploaded
/// it. A new version is what the file is read as from then on; the one before stays to
/// be read; and the upload that became the version is no file of its own.
#[tokio::test]
async fn a_new_version_is_what_the_file_is_and_the_one_before_stays() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_file(&ts).await;
    let (user1, user2) = (ts.client.me().await.id, ts.client2.me().await.id);

    let only = versions(&ts.client, &board.id, &file_id).await;
    assert_eq!(said(&only), [(1, true, None)]);
    assert_eq!(only[0]["createdBy"], json!(user1));
    assert_eq!(only[0]["size"], json!(5));
    assert_eq!(
        version_bytes(&ts.client, &board.id, &file_id, 1).await,
        b"first"
    );

    // The second user edits the open board once they joined it.
    ts.client2
        .post(
            &format!("{TEST_WS}/containers/{}/join", board.id),
            &json!({}),
        )
        .await
        .check_ok();
    let (added, uploaded) = add_version(&ts.client2, &board.id, &file_id, 1, b"the second").await;
    let made: Value = added.created_json();
    assert_eq!(
        (&made["version"], &made["isCurrent"], &made["createdBy"]),
        (&json!(2), &json!(true), &json!(user2))
    );
    assert_eq!(made["size"], json!(10));
    assert_eq!(
        file_bytes(&ts.client, &board.id, &file_id).await,
        b"the second"
    );
    let info: Value = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{file_id}/info",
            board.id
        ))
        .await
        .ok_json();
    assert_eq!(
        (&info["size"], &info["name"]),
        (&json!(10), &json!("a.txt")),
        "the file keeps its name"
    );
    let both = versions(&ts.client, &board.id, &file_id).await;
    assert_eq!(said(&both), [(2, true, None), (1, false, None)]);
    assert_eq!(both[1]["createdBy"], json!(user1));
    assert_eq!(
        version_bytes(&ts.client, &board.id, &file_id, 1).await,
        b"first"
    );
    assert_eq!(
        version_bytes(&ts.client, &board.id, &file_id, 2).await,
        b"the second"
    );
    for path in [
        format!("{TEST_WS}/containers/{}/files/{uploaded}", board.id),
        format!("{TEST_WS}/containers/{}/files/{uploaded}/info", board.id),
        versions_path(&board.id, &uploaded),
    ] {
        ts.client.get(&path).await.check_not_found();
    }
    for missing in ["3", "0", "-1", "one"] {
        let path = format!("{}/{missing}", versions_path(&board.id, &file_id));
        let answer = ts.client.get(&path).await;
        assert!(
            matches!(
                answer.status,
                StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST
            ),
            "{missing}: {}",
            answer.status
        );
    }
    ts.teardown().await;
}

/// Going back to a version writes its bytes as a new version, by who went back, that
/// names the one gone back to; the versions between stay. Going back to the newest
/// changes nothing.
#[tokio::test]
async fn going_back_to_a_version_writes_a_new_one() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_file(&ts).await;
    add_version(&ts.client, &board.id, &file_id, 1, b"the second")
        .await
        .0
        .check_created();

    let restored: Value = restore(&ts.client, &board.id, &file_id, 1, 2)
        .await
        .ok_json();
    assert_eq!(
        (
            &restored["version"],
            &restored["restoredFrom"],
            &restored["isCurrent"]
        ),
        (&json!(3), &json!(1), &json!(true))
    );
    assert_eq!(file_bytes(&ts.client, &board.id, &file_id).await, b"first");
    assert_eq!(
        said(&versions(&ts.client, &board.id, &file_id).await),
        [(3, true, Some(1)), (2, false, None), (1, false, None)]
    );
    let kept = places(&ts, &board.id, &file_id).await;
    assert_ne!(kept[0], kept[2], "the bytes gone back to are a copy");

    let same: Value = restore(&ts.client, &board.id, &file_id, 3, 3)
        .await
        .ok_json();
    assert_eq!(same["version"], json!(3), "the newest is left as it is");
    assert_eq!(versions(&ts.client, &board.id, &file_id).await.len(), 3);
    restore(&ts.client, &board.id, &file_id, 9, 3)
        .await
        .check_not_found();
    restore(&ts.client, &board.id, "nothing.txt", 1, 1)
        .await
        .check_not_found();
    ts.teardown().await;
}

/// A version is written from the version the request names. If the file has a newer
/// one, nothing changes: 409 with the reason, and the upload stays a file of the board.
#[tokio::test]
async fn a_version_written_from_an_older_one_is_a_conflict() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_file(&ts).await;
    add_version(&ts.client, &board.id, &file_id, 1, b"the second")
        .await
        .0
        .check_created();

    for base in [1, 3, 0] {
        let (late, uploaded) = add_version(&ts.client, &board.id, &file_id, base, b"late").await;
        check_stale(&late);
        assert_eq!(
            file_bytes(&ts.client, &board.id, &uploaded).await,
            b"late",
            "the upload is still a file"
        );
        check_stale(&restore(&ts.client, &board.id, &file_id, 1, base).await);
    }
    assert_eq!(
        said(&versions(&ts.client, &board.id, &file_id).await),
        [(2, true, None), (1, false, None)]
    );
    assert_eq!(
        file_bytes(&ts.client, &board.id, &file_id).await,
        b"the second"
    );
    ts.teardown().await;
}

/// Of two versions written at once from the same version, one is written and the
/// other is a conflict whose upload stays a file. Both files are up before: only the
/// two writes are made together.
#[tokio::test]
async fn of_two_versions_written_at_once_one_is_a_conflict() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_file(&ts).await;
    let path = versions_path(&board.id, &file_id);
    for round in 1..=5 {
        let one = uploaded_id(&upload(&ts.client, &board.id, "next.txt", b"one").await);
        let other = uploaded_id(&upload(&ts.client, &board.id, "next.txt", b"other").await);
        let from_one = json!({"baseVersion": round, "fileId": one});
        let from_other = json!({"baseVersion": round, "fileId": other});
        let (first, second) = tokio::join!(
            ts.client.post(&path, &from_one),
            ts.client.post(&path, &from_other)
        );
        let (lost, lost_upload, won_bytes) = match (first.status, second.status) {
            (StatusCode::CREATED, StatusCode::CONFLICT) => (second, other, &b"one"[..]),
            (StatusCode::CONFLICT, StatusCode::CREATED) => (first, one, &b"other"[..]),
            statuses => panic!("round {round}: {statuses:?}"),
        };
        check_stale(&lost);
        assert_eq!(file_bytes(&ts.client, &board.id, &file_id).await, won_bytes);
        file_bytes(&ts.client, &board.id, &lost_upload).await;
        assert_eq!(
            versions(&ts.client, &board.id, &file_id).await.len(),
            usize::try_from(round).expect("small") + 1
        );
    }
    ts.teardown().await;
}

/// A version gone back to and one written at once from the same version: one of the
/// two is the file's new version, the other a conflict, and no number is given twice.
#[tokio::test]
async fn of_a_version_gone_back_to_and_one_written_at_once_one_is_a_conflict() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_file(&ts).await;
    add_version(&ts.client, &board.id, &file_id, 1, b"the second")
        .await
        .0
        .check_created();
    let path = versions_path(&board.id, &file_id);
    for base in 2..=6 {
        let next = uploaded_id(&upload(&ts.client, &board.id, "next.txt", b"written").await);
        let written = json!({"baseVersion": base, "fileId": next});
        let (back, added) = tokio::join!(
            restore(&ts.client, &board.id, &file_id, 1, base),
            ts.client.post(&path, &written)
        );
        let (lost, won_bytes) = match (back.status, added.status) {
            (StatusCode::OK, StatusCode::CONFLICT) => (added, &b"first"[..]),
            (StatusCode::CONFLICT, StatusCode::CREATED) => (back, &b"written"[..]),
            statuses => panic!("from {base}: {statuses:?}"),
        };
        check_stale(&lost);
        assert_eq!(file_bytes(&ts.client, &board.id, &file_id).await, won_bytes);
        let numbers: Vec<i64> = said(&versions(&ts.client, &board.id, &file_id).await)
            .iter()
            .map(|version| version.0)
            .collect();
        let counted: Vec<i64> = (1..=base + 1).rev().collect();
        assert_eq!(numbers, counted, "from {base}");
    }
    ts.teardown().await;
}

/// The attachments that give a file say the size of its newest version: as they are
/// made, once one is written and once one is gone back to; the attachment of another
/// file is left.
#[tokio::test]
async fn an_attachment_says_the_size_of_the_newest_version() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_file(&ts).await;
    let other = uploaded_id(&upload(&ts.client, &board.id, "other.txt", b"another").await);
    attach(&ts, &board.id, &file_id).await;
    attach(&ts, &board.id, &other).await;
    assert_eq!(attached_size(&ts, &board.id, &file_id).await, Some(5));
    assert_eq!(attached_size(&ts, &board.id, &other).await, Some(7));

    add_version(&ts.client, &board.id, &file_id, 1, b"the second")
        .await
        .0
        .check_created();
    assert_eq!(attached_size(&ts, &board.id, &file_id).await, Some(10));
    restore(&ts.client, &board.id, &file_id, 1, 2)
        .await
        .check_ok();
    assert_eq!(attached_size(&ts, &board.id, &file_id).await, Some(5));
    assert_eq!(attached_size(&ts, &board.id, &other).await, Some(7));
    ts.teardown().await;
}

/// The versions of a file a card attaches stay while the card is in the trash and
/// are there when it comes back; once the card is removed for good and the file is
/// collected, the versions and their objects are gone.
#[tokio::test]
async fn versions_go_for_good_with_the_card_that_attached_their_file() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_file(&ts).await;
    let card_id = attach(&ts, &board.id, &file_id).await;
    add_version(&ts.client, &board.id, &file_id, 1, b"the second")
        .await
        .0
        .check_created();
    restore(&ts.client, &board.id, &file_id, 1, 2)
        .await
        .check_ok();
    let kept = places(&ts, &board.id, &file_id).await;
    assert_eq!(kept.len(), 3, "{kept:?}");
    let me = ts.client.me().await.id;
    let collect = async || {
        let store = ts.app.store();
        let collected = store.collect_unreferenced_files(i64::MAX).await;
        collected.expect("collected").len()
    };

    ts.app
        .delete_block_unchecked(&card_id, &me, ChangeNotification::default())
        .await
        .expect("the card goes to the trash");
    assert_eq!(collect().await, 0, "the card in the trash holds its file");
    assert_eq!(remove_doomed(&ts, now_millis()).await.removed, 0);
    ts.app
        .undelete_block_unchecked(&card_id, &me)
        .await
        .expect("the card comes back");
    assert_eq!(versions(&ts.client, &board.id, &file_id).await.len(), 3);
    assert_eq!(
        version_bytes(&ts.client, &board.id, &file_id, 2).await,
        b"the second"
    );

    ts.app
        .delete_block_unchecked(&card_id, &me, ChangeNotification::default())
        .await
        .expect("the card goes to the trash again");
    ts.app
        .store()
        .purge_trash(i64::MAX)
        .await
        .expect("the trash is purged");
    assert_eq!(collect().await, 1, "the file nothing shows any more");
    assert_eq!(remove_doomed(&ts, now_millis()).await.removed, 3);
    for path in &kept {
        assert!(!exists(&ts, path).await, "{path} went with the card");
    }
    ts.client
        .get(&versions_path(&board.id, &file_id))
        .await
        .check_not_found();
    ts.teardown().await;
}

/// What cannot become a version is refused at `/fileId`, and nothing changes: a file
/// the board does not have, the file itself, one of another kind, one a block shows,
/// one with versions of its own, and a file of another board.
#[tokio::test]
async fn an_upload_that_cannot_become_a_version_is_refused() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_file(&ts).await;
    let up = |name: &'static str| {
        let (client, board_id) = (&ts.client, board.id.clone());
        async move { uploaded_id(&upload(client, &board_id, name, b"bytes").await) }
    };
    let other_kind = up("b.pdf").await;
    let shown = up("shown.txt").await;
    attach(&ts, &board.id, &shown).await;
    let versioned = up("versioned.txt").await;
    add_version(&ts.client, &board.id, &versioned, 1, b"again")
        .await
        .0
        .check_created();
    let other_board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let elsewhere = uploaded_id(&upload(&ts.client, &other_board.id, "c.txt", b"bytes").await);

    for (what, upload_id) in [
        ("missing", "nothing.txt"),
        ("itself", file_id.as_str()),
        ("of another kind", other_kind.as_str()),
        ("shown", shown.as_str()),
        ("with versions", versioned.as_str()),
        ("of another board", elsewhere.as_str()),
    ] {
        let answer = ts
            .client
            .post(
                &versions_path(&board.id, &file_id),
                &json!({"baseVersion": 1, "fileId": upload_id}),
            )
            .await;
        answer.check_bad_request();
        assert_eq!(answer.refused_at(), "/fileId", "{what}");
    }
    for body in [json!({"fileId": other_kind}), json!({"baseVersion": 1})] {
        ts.client
            .post(&versions_path(&board.id, &file_id), &body)
            .await
            .check_bad_request();
    }
    assert_eq!(
        said(&versions(&ts.client, &board.id, &file_id).await),
        [(1, true, None)]
    );
    for kept in [&other_kind, &shown, &versioned] {
        file_bytes(&ts.client, &board.id, kept).await;
    }
    ts.teardown().await;
}

/// Who reads the board reads the versions and writes none; who is not on it gets
/// nothing; and nobody reads a version by the board's link: the routes of the board
/// take no read token, and the shared ones have none for versions.
#[tokio::test]
async fn versions_are_read_by_readers_and_written_by_editors() {
    // Public links are allowed, so that the link is one a reader holds.
    let ts = Setup::local_mode().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let file_id = uploaded_id(&upload(&ts.client, &board.id, "a.txt", b"first").await);
    add_version(&ts.client, &board.id, &file_id, 1, b"the second")
        .await
        .0
        .check_created();
    let spare = uploaded_id(&upload(&ts.client, &board.id, "spare.txt", b"spare").await);
    let list = versions_path(&board.id, &file_id);
    let write = json!({"baseVersion": 2, "fileId": spare});

    // Not on the board.
    for path in [list.clone(), format!("{list}/1")] {
        let answer = ts.client2.get(&path).await;
        assert!(
            matches!(answer.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "{path}: {}",
            answer.status
        );
    }
    assert_ne!(
        ts.client2.post(&list, &write).await.status,
        StatusCode::CREATED
    );

    let viewer = fvoci_model::ContainerMember {
        container_id: board.id.clone(),
        user_id: ts.client2.me().await.id,
        role: fvoci_model::ContainerRole::Viewer,
        synthetic: false,
        inherited_role: None,
    };
    ts.client.add_member(&viewer).await.check_ok();
    assert_eq!(versions(&ts.client2, &board.id, &file_id).await.len(), 2);
    assert_eq!(
        version_bytes(&ts.client2, &board.id, &file_id, 1).await,
        b"first"
    );
    ts.client2.post(&list, &write).await.check_forbidden();
    restore(&ts.client2, &board.id, &file_id, 1, 2)
        .await
        .check_forbidden();

    // By the link: the newest version, and no way to the ones before.
    let token = ts.share(&board.id).await;
    let anonymous = ts.anonymous();
    let shared = format!("/shared/{}/files/{file_id}", board.id);
    assert_eq!(
        bytes_at(&anonymous, &format!("{shared}?readToken={token}")).await,
        b"the second"
    );
    for path in [
        format!("{shared}/versions?readToken={token}"),
        format!("{shared}/versions/1?readToken={token}"),
    ] {
        anonymous.get(&path).await.check_not_found();
    }
    for path in [
        format!("{list}?readToken={token}"),
        format!("{list}/1?readToken={token}"),
    ] {
        anonymous.get(&path).await.check_unauthorized();
    }
    assert_eq!(
        said(&versions(&ts.client, &board.id, &file_id).await),
        [(2, true, None), (1, false, None)],
        "nothing was written"
    );
    ts.teardown().await;
}

/// The server keeps the newest versions of a file, as many as it is set to: the
/// oldest goes as a new one comes, and its object with it.
#[tokio::test]
async fn the_oldest_version_goes_when_more_are_written_than_are_kept() {
    let ts = Setup::native()
        .configure(|config| config.max_file_versions = 3)
        .start()
        .await
        .init_basic()
        .await;
    let (board, file_id) = board_with_file(&ts).await;
    for base in 1..=2 {
        add_version(&ts.client, &board.id, &file_id, base, b"more")
            .await
            .0
            .check_created();
    }
    let three = places(&ts, &board.id, &file_id).await;
    assert_eq!(three.len(), 3);
    add_version(&ts.client, &board.id, &file_id, 3, b"the fourth")
        .await
        .0
        .check_created();

    assert_eq!(
        said(&versions(&ts.client, &board.id, &file_id).await),
        [(4, true, None), (3, false, None), (2, false, None)]
    );
    let path = format!("{}/1", versions_path(&board.id, &file_id));
    ts.client.get(&path).await.check_not_found();
    restore(&ts.client, &board.id, &file_id, 1, 4)
        .await
        .check_not_found();
    let first = &three[2];
    assert!(exists(&ts, first).await, "doomed, and there until removed");
    assert_eq!(remove_doomed(&ts, now_millis()).await.removed, 1);
    assert!(
        !exists(&ts, first).await,
        "the oldest version's object went"
    );
    for kept in places(&ts, &board.id, &file_id).await {
        assert!(exists(&ts, &kept).await, "{kept}");
    }
    ts.teardown().await;
}

/// Unless set otherwise the server keeps fifty versions of a file: the fifty-first
/// drops the first.
#[tokio::test]
async fn the_fifty_first_version_drops_the_first() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_file(&ts).await;
    for base in 1..=49 {
        add_version(&ts.client, &board.id, &file_id, base, b"more")
            .await
            .0
            .check_created();
    }
    let fifty = said(&versions(&ts.client, &board.id, &file_id).await);
    assert_eq!(
        (fifty.len(), fifty[0].0, fifty[49].0),
        (50, 50, 1),
        "fifty are kept"
    );
    add_version(&ts.client, &board.id, &file_id, 50, b"the fifty-first")
        .await
        .0
        .check_created();
    let kept = said(&versions(&ts.client, &board.id, &file_id).await);
    assert_eq!((kept.len(), kept[0].0, kept[49].0), (50, 51, 2));
    ts.teardown().await;
}

/// The versions of a file stay with its board in the trash and come back with it; when
/// the board goes for good the objects of every version go, and no other file's.
#[tokio::test]
async fn versions_go_to_the_trash_and_for_good_with_their_board() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_file(&ts).await;
    attach(&ts, &board.id, &file_id).await;
    add_version(&ts.client, &board.id, &file_id, 1, b"the second")
        .await
        .0
        .check_created();
    restore(&ts.client, &board.id, &file_id, 1, 2)
        .await
        .check_ok();
    let kept = places(&ts, &board.id, &file_id).await;
    assert_eq!(kept.len(), 3);
    let (other_board, other_file) = board_with_file(&ts).await;
    add_version(&ts.client, &other_board.id, &other_file, 1, b"another")
        .await
        .0
        .check_created();
    let others = places(&ts, &other_board.id, &other_file).await;

    let board_path = format!("{TEST_WS}/containers/{}", board.id);
    ts.client.delete(&board_path).await.check_no_content();
    ts.client
        .get(&versions_path(&board.id, &file_id))
        .await
        .check_not_found();
    // Nothing of a board in the trash is collected or removed.
    ts.app
        .store()
        .collect_unreferenced_files(0)
        .await
        .expect("collected");
    assert_eq!(remove_doomed(&ts, now_millis()).await.removed, 0);
    ts.client
        .post(&format!("{board_path}/restore"), &json!({}))
        .await
        .check_ok();
    assert_eq!(
        said(&versions(&ts.client, &board.id, &file_id).await),
        [(3, true, Some(1)), (2, false, None), (1, false, None)],
        "back as they were"
    );
    assert_eq!(
        version_bytes(&ts.client, &board.id, &file_id, 2).await,
        b"the second"
    );
    // A version written after the board came back counts on from where it was.
    add_version(&ts.client, &board.id, &file_id, 3, b"the fourth")
        .await
        .0
        .check_created();
    let kept = [kept, places(&ts, &board.id, &file_id).await].concat();

    ts.client.delete(&board_path).await.check_no_content();
    ts.app
        .store()
        .purge_trash(i64::MAX)
        .await
        .expect("the trash is purged");
    remove_doomed(&ts, now_millis()).await;
    for path in &kept {
        assert!(!exists(&ts, path).await, "{path} went with the board");
    }
    for path in &others {
        assert!(exists(&ts, path).await, "{path} is another board's");
    }
    assert_eq!(
        said(&versions(&ts.client, &other_board.id, &other_file).await),
        [(2, true, None), (1, false, None)]
    );
    ts.teardown().await;
}

/// A file that no block shows goes with all its versions once it is collected; one a
/// block shows keeps them.
#[tokio::test]
async fn a_file_no_block_shows_is_collected_with_its_versions() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, unshown) = board_with_file(&ts).await;
    let shown = uploaded_id(&upload(&ts.client, &board.id, "shown.txt", b"first").await);
    attach(&ts, &board.id, &shown).await;
    for file_id in [&unshown, &shown] {
        add_version(&ts.client, &board.id, file_id, 1, b"the second")
            .await
            .0
            .check_created();
    }
    let gone = places(&ts, &board.id, &unshown).await;
    let kept = places(&ts, &board.id, &shown).await;
    assert_eq!((gone.len(), kept.len()), (2, 2));

    let collected = ts
        .app
        .store()
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("collected");
    assert_eq!(collected.len(), 1, "{collected:?}");
    assert_eq!(remove_doomed(&ts, now_millis()).await.removed, 2);
    for path in &gone {
        assert!(!exists(&ts, path).await, "{path}");
    }
    for path in &kept {
        assert!(exists(&ts, path).await, "{path}");
    }
    ts.client
        .get(&versions_path(&board.id, &unshown))
        .await
        .check_not_found();
    assert_eq!(versions(&ts.client, &board.id, &shown).await.len(), 2);
    ts.teardown().await;
}

/// The object of an older version is held by its row: the comparison of the records
/// with the file storage finds nothing unrecorded, and a doomed place that a version
/// names is not removed.
#[tokio::test]
async fn the_objects_of_older_versions_are_recorded_and_held() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_file(&ts).await;
    attach(&ts, &board.id, &file_id).await;
    for base in 1..=2 {
        add_version(&ts.client, &board.id, &file_id, base, b"more")
            .await
            .0
            .check_created();
    }
    let kept = places(&ts, &board.id, &file_id).await;

    let found = ts
        .app
        .housekeeping()
        .reconcile_files(
            fvoci_server::app::Unrecorded::Doom,
            now_millis() + 2 * fvoci_server::app::UNRECORDED_MIN_AGE_MILLIS,
        )
        .await
        .expect("compared");
    assert_eq!(found.unrecorded.count, 0, "{found:?}");
    assert_eq!(found.missing.count, 0, "{found:?}");

    let oldest = kept[2].as_str();
    let held = ts
        .app
        .store()
        .held_objects(&[oldest], None)
        .await
        .expect("asked");
    assert!(held.contains(oldest), "{held:?}");
    ts.app
        .store()
        .mark_writing(oldest, 0)
        .await
        .expect("marked, as a wrong call could");
    let removed = remove_doomed(&ts, now_millis()).await;
    assert_eq!((removed.removed, removed.kept), (0, 1));
    assert_eq!(
        version_bytes(&ts.client, &board.id, &file_id, 1).await,
        b"first"
    );
    ts.teardown().await;
}

/// A copy of a card takes the newest version of each file, as a file of its own with
/// one version; and an archive of the board carries the newest version only.
#[tokio::test]
async fn a_copy_and_an_archive_carry_the_newest_version_only() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board, file_id) = board_with_file(&ts).await;
    attach(&ts, &board.id, &file_id).await;
    add_version(&ts.client, &board.id, &file_id, 1, b"the second")
        .await
        .0
        .check_created();

    let export = ts
        .client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", board.id))
        .await;
    export.check_ok();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(export.body.clone()))
        .expect("the export is an archive");
    let names: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .filter(|name| name.ends_with(".txt"))
        .map(std::borrow::Cow::into_owned)
        .collect();
    assert_eq!(names.len(), 1, "one entry for the file: {names:?}");
    let mut carried = Vec::new();
    std::io::Read::read_to_end(
        &mut archive.by_name(&names[0]).expect("the entry"),
        &mut carried,
    )
    .expect("read");
    assert_eq!(carried, b"the second");

    let copy: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/duplicate", board.id),
            &json!({}),
        )
        .await
        .created_json();
    let copy_id = copy["containers"][0]["id"].as_str().expect("the copy");
    let blocks = crate::blocks::blocks_of(&ts.client, copy_id).await;
    let copied = blocks
        .iter()
        .find(|block| block.block_type == BlockType::Attachment)
        .and_then(|block| block.fields.get("fileId"))
        .and_then(Value::as_str)
        .expect("the copy's attachment");
    assert_ne!(copied, file_id);
    assert_eq!(file_bytes(&ts.client, copy_id, copied).await, b"the second");
    assert_eq!(
        said(&versions(&ts.client, copy_id, copied).await),
        [(1, true, None)]
    );
    ts.teardown().await;
}

/// A wiki's file has versions as a board's has.
#[tokio::test]
async fn a_file_of_a_wiki_has_versions() {
    let ts = Setup::native().start().await.init_basic().await;
    let wiki: Container = ts
        .client
        .post(
            &format!("{TEST_WS}/wikis"),
            &json!({"visibility": "private"}),
        )
        .await
        .created_json();
    let file_id = uploaded_id(&upload(&ts.client, &wiki.id, "a.txt", b"first").await);
    add_version(&ts.client, &wiki.id, &file_id, 1, b"the second")
        .await
        .0
        .check_created();
    restore(&ts.client, &wiki.id, &file_id, 1, 2)
        .await
        .check_ok();
    assert_eq!(
        said(&versions(&ts.client, &wiki.id, &file_id).await),
        [(3, true, Some(1)), (2, false, None), (1, false, None)]
    );
    assert_eq!(file_bytes(&ts.client, &wiki.id, &file_id).await, b"first");
    ts.client2
        .get(&versions_path(&wiki.id, &file_id))
        .await
        .check_not_found();
    ts.teardown().await;
}

/// The versions of a file of another organization are not there, under whatever
/// workspace they are asked; and what removes the versions of one organization's files
/// leaves the other's.
#[tokio::test]
async fn the_versions_of_two_organizations_stay_apart() {
    // SQLite holds one organization.
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native().start().await.init_basic().await;
    let (theirs, their_workspace, their_board) = crate::workspaces::another_organization(&ts).await;
    let record = |name: &str| {
        let stored_name = format!("{}.txt", fvoci_server::auth::secret::new_secret());
        FileRecord {
            id: new_id(),
            container_id: their_board.clone(),
            path: format!("boards/20261118/{their_board}/{stored_name}"),
            stored_name,
            name: name.to_owned(),
            extension: ".txt".to_owned(),
            size: 6,
            created_at: 1,
            ..FileRecord::default()
        }
    };
    let (file, next) = (record("theirs.txt"), record("next.txt"));
    for info in [&file, &next] {
        ts.app
            .files()
            .write(&info.path, &mut &b"theirs"[..])
            .await
            .expect("written");
        theirs.save_file_record(info).await.expect("recorded");
    }
    let owner = theirs
        .get_members_for_container(&their_board)
        .await
        .expect("their members")[0]
        .user_id
        .clone();
    let added = theirs
        .add_file_version(
            &their_board,
            &file.stored_name,
            1,
            &next.stored_name,
            &owner,
        )
        .await
        .expect("their version is written");
    assert!(matches!(added, VersionAdded::Written(_)), "{added:?}");

    for workspace in [TEST_WORKSPACE_ID, their_workspace.as_str()] {
        let list = format!(
            "/w/{workspace}/containers/{their_board}/files/{}/versions",
            file.stored_name
        );
        ts.client.get(&list).await.check_not_found();
        ts.client.get(&format!("{list}/1")).await.check_not_found();
        ts.client
            .post(&list, &json!({"baseVersion": 2, "fileId": "x.txt"}))
            .await
            .check_not_found();
        ts.client
            .post(&format!("{list}/1/restore"), &json!({"baseVersion": 2}))
            .await
            .check_not_found();
    }

    // Our board goes for good with a versioned file of its own: theirs stay.
    let (board, file_id) = board_with_file(&ts).await;
    add_version(&ts.client, &board.id, &file_id, 1, b"ours")
        .await
        .0
        .check_created();
    let ours = places(&ts, &board.id, &file_id).await;
    ts.client
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await
        .check_no_content();
    ts.app
        .store()
        .purge_trash(i64::MAX)
        .await
        .expect("the trash is purged");
    ts.app
        .store()
        .collect_unreferenced_files(0)
        .await
        .expect("collected");
    remove_doomed(&ts, now_millis()).await;
    for path in &ours {
        assert!(!exists(&ts, path).await, "{path}");
    }
    let kept = theirs
        .file_versions(&their_board, &file.stored_name)
        .await
        .expect("their versions");
    assert_eq!(kept.len(), 2);
    for version in &kept {
        assert!(exists(&ts, &version.path).await, "{}", version.path);
    }
    ts.teardown().await;
}
