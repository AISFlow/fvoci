//! The objects of the file storage whose record went: removed by the housekeeping,
//! tried again when the removal fails, and finished when a run was cut off. And what
//! keeps an object: a record of any board or organization.

use fvoci_model::{ContainerVisibility, FileRecord, Upload, UploadPurpose, Workspace, new_id};
use fvoci_server::app::{RemovedObjects, WRITING_GRACE_MILLIS};
use fvoci_server::auth::now_millis;
use fvoci_server::auth::secret::new_secret;
use fvoci_server::storage::FaultyOp;
use fvoci_store::testing::db_type_of_the_tests;
use fvoci_store::{DbType, LooseState, Store};
use tokio::io::AsyncWriteExt as _;

use crate::common::TEST_WS;
use crate::common::{Setup, TEST_WORKSPACE_ID, TestServer};
use crate::files::{key_of, upload, uploaded_id};
use crate::metrics::{sample, scrape};
use crate::uploads::s3_server;

const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// Uploads a file to a new board and returns the board and the file's place in the
/// file storage.
pub(crate) async fn uploaded(ts: &TestServer, data: &[u8]) -> (String, String) {
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = upload(&ts.client, &board.id, "a.txt", data).await;
    response.check_created();
    let info = ts
        .app
        .store()
        .get_file_record(&board.id, &uploaded_id(&response))
        .await
        .expect("the file is recorded");
    (board.id, info.path)
}

/// The stored name of a file [`uploaded`] made at `path`: the secret its key ends in,
/// with the extension of `a.txt`.
fn stored_name_at(path: &str) -> String {
    format!("{}.txt", path.rsplit('/').next().unwrap_or_default())
}

pub(crate) async fn exists(ts: &TestServer, path: &str) -> bool {
    ts.app
        .files()
        .exists(path)
        .await
        .expect("the storage is read")
}

pub(crate) async fn remove_doomed(ts: &TestServer, now: i64) -> RemovedObjects {
    ts.app
        .housekeeping()
        .remove_doomed_objects(now)
        .await
        .expect("the doomed objects are read")
}

/// The objects of the records that the collection of unshown files and the purge of the
/// trash remove are removed by the housekeeping; one whose removal fails stays listed,
/// with the failure noted, until it is due again and goes.
async fn doomed_objects_are_removed_and_tried_again(ts: &TestServer) {
    let housekeeping = ts.app.housekeeping();
    let (_, unshown) = uploaded(ts, b"shown by no block").await;
    let (board_id, purged) = uploaded(ts, b"of a purged board").await;
    ts.client
        .delete(&format!("{TEST_WS}/containers/{board_id}"))
        .await
        .check_no_content();
    housekeeping
        .store()
        .purge_trash(i64::MAX)
        .await
        .expect("the trash is purged");
    housekeeping
        .store()
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("the files no block shows are collected");
    assert!(
        exists(ts, &unshown).await && exists(ts, &purged).await,
        "the objects stay until the housekeeping removes them"
    );

    let faults = ts
        .app
        .files()
        .faults()
        .expect("a test's storage takes faults");
    faults.fail(FaultyOp::Delete, 1);
    let now = now_millis();
    let first = remove_doomed(ts, now).await;
    assert_eq!(
        first,
        RemovedObjects {
            removed: 1,
            failed: 1,
            kept: 0,
        },
        "one removal fails"
    );
    let listed = housekeeping.store().loose_objects().await.expect("listed");
    assert_eq!(listed.len(), 1, "{listed:?}");
    let failed = &listed[0];
    assert!(exists(ts, &failed.path).await, "its object is there");
    assert_eq!(
        (failed.state, failed.attempts, failed.last_error.as_str()),
        (LooseState::Doomed, 1, "io"),
        "the failure is noted"
    );
    assert!(failed.retry_at > now, "tried again later: {failed:?}");

    assert_eq!(
        remove_doomed(ts, now).await,
        RemovedObjects::default(),
        "not before it is due"
    );
    assert_eq!(
        remove_doomed(ts, now + DAY_MILLIS).await,
        RemovedObjects {
            removed: 1,
            failed: 0,
            kept: 0,
        },
        "and then it goes"
    );
    assert!(
        !exists(ts, &unshown).await && !exists(ts, &purged).await,
        "both objects are gone"
    );
    assert_eq!(
        housekeeping.store().loose_objects().await.expect("listed"),
        [],
        "nothing is left listed"
    );
}

/// A run cut off between the removal of an object and taking it off the list leaves it
/// listed; the next run finds it gone and takes it off.
async fn a_run_cut_off_after_the_removal_is_finished_by_the_next(ts: &TestServer) {
    let housekeeping = ts.app.housekeeping();
    let (_, path) = uploaded(ts, b"removed, still listed").await;
    housekeeping
        .store()
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("collected");
    ts.app
        .files()
        .delete(&path)
        .await
        .expect("the removal of the run that was cut off");
    assert_eq!(
        remove_doomed(ts, now_millis()).await,
        RemovedObjects {
            removed: 1,
            failed: 0,
            kept: 0,
        },
        "the object is found gone"
    );
    assert_eq!(
        housekeeping.store().loose_objects().await.expect("listed"),
        [],
        "and taken off the list"
    );
}

#[tokio::test]
async fn doomed_objects_on_the_disk_are_removed_and_tried_again() {
    let ts = Setup::native().start().await.init_basic().await;
    doomed_objects_are_removed_and_tried_again(&ts).await;
    a_run_cut_off_after_the_removal_is_finished_by_the_next(&ts).await;
    ts.teardown().await;
}

#[tokio::test]
#[ignore = "needs an S3-compatible server; see tests/storage.rs"]
async fn with_s3_doomed_objects_are_removed_and_tried_again() {
    let ts = s3_server().await;
    doomed_objects_are_removed_and_tried_again(&ts).await;
    a_run_cut_off_after_the_removal_is_finished_by_the_next(&ts).await;
    ts.teardown().await;
}

/// Writes a file of `board_id` and records it through `owner`, and returns its place.
async fn recorded_file(ts: &TestServer, owner: &Store, board_id: &str) -> String {
    let secret = new_secret();
    let stored_name = format!("{secret}.txt");
    let path = key_of(ts, board_id, &secret).await;
    ts.app
        .files()
        .write(&path, &mut &b"recorded"[..])
        .await
        .expect("the file is written");
    owner
        .save_file_record(&FileRecord {
            id: new_id(),
            container_id: board_id.to_owned(),
            stored_name,
            path: path.clone(),
            size: 8,
            created_at: 1,
            ..FileRecord::default()
        })
        .await
        .expect("the file is recorded");
    path
}

/// An upload session of the second user, on a board of the test's organization, that
/// makes its file at `path`: as only a wrong call of the store could store one.
async fn misbound_upload(ts: &TestServer, path: &str) -> Upload {
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let upload = Upload {
        id: new_secret(),
        user_id: ts.client2.me().await.id,
        purpose: UploadPurpose::File,
        container_id: board.id,
        name: "misbound.txt".to_owned(),
        size: 8,
        part_size: 8,
        created_at: now_millis(),
        expires_at: now_millis() + 60_000,
        multipart_id: "left-over-multipart".to_owned(),
        path: path.to_owned(),
        ..Upload::default()
    };
    ts.app
        .store()
        .create_upload(&upload)
        .await
        .expect("the session is stored");
    upload
}

/// Abandons a session that makes its file at the place of a recorded file of another
/// board, and checks that the file is still there.
async fn abandoning_it_keeps_the_file(ts: &TestServer, path: &str) {
    let upload = misbound_upload(ts, path).await;
    ts.client2
        .delete(&format!("{TEST_WS}/uploads/{}", upload.id))
        .await
        .check_no_content();
    assert!(
        exists(ts, path).await,
        "the recorded file is still there after the session that named its place went"
    );
}

/// A session that names the place of another board's recorded file does not take the
/// file with it when it is abandoned: whether an object may go is asked of every
/// record, whatever board the session names.
#[tokio::test]
async fn abandoning_a_misbound_upload_keeps_another_boards_file() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let path = recorded_file(&ts, ts.app.store(), &board.id).await;
    abandoning_it_keeps_the_file(&ts, &path).await;
    ts.teardown().await;
}

/// The same across organizations: a session of one organization that names the place
/// of another organization's file does not take it with it (an object's place is one
/// across organizations).
#[tokio::test]
async fn abandoning_a_misbound_upload_keeps_another_organizations_file() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native().start().await.init_basic().await;
    let (theirs, board_id) = another_organizations_board(&ts).await;
    let path = recorded_file(&ts, &theirs, &board_id).await;
    abandoning_it_keeps_the_file(&ts, &path).await;
    ts.teardown().await;
}

/// A second organization, a board of it, and the handle that reaches its rows.
async fn another_organizations_board(ts: &TestServer) -> (Store, String) {
    let workspace = Workspace {
        id: new_id(),
        ..Workspace::default()
    };
    let org_id = ts
        .app
        .store()
        .create_organization("another", &workspace)
        .await
        .expect("a second organization is made");
    let theirs = ts.app.store().for_org(&org_id).expect("an identifier");
    let mut board = fvoci_model::Container {
        id: new_id(),
        workspace_id: workspace.id.clone(),
        visibility: ContainerVisibility::Open,
        ..fvoci_model::Container::default()
    };
    theirs
        .insert_container(&mut board, &ts.client.me().await.id)
        .await
        .expect("their board is stored");
    (theirs, board.id)
}

/// The test's organization dooms the place of a recorded file, as a wrong call of the
/// store could (a mark of writing at the place, gone stale): the housekeeping asks of
/// every organization before it removes anything, keeps the file, takes the row off the
/// list and counts it as an error.
async fn a_doomed_object_that_is_held_is_kept(ts: &TestServer, path: &str) {
    ts.app
        .store()
        .mark_writing(path, 0)
        .await
        .expect("the place is marked");
    assert_eq!(
        remove_doomed(ts, now_millis()).await,
        RemovedObjects {
            removed: 0,
            failed: 0,
            kept: 1,
        },
        "the held object is kept, as an error"
    );
    assert!(exists(ts, path).await, "the recorded file is still there");
    assert_eq!(
        ts.app
            .housekeeping()
            .store()
            .loose_objects()
            .await
            .expect("listed"),
        [],
        "and taken off the list"
    );
}

#[tokio::test]
async fn a_doomed_object_that_another_board_holds_is_kept() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let path = recorded_file(&ts, ts.app.store(), &board.id).await;
    a_doomed_object_that_is_held_is_kept(&ts, &path).await;
    ts.teardown().await;
}

#[tokio::test]
async fn a_doomed_object_that_another_organization_holds_is_kept() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native().start().await.init_basic().await;
    let (theirs, board_id) = another_organizations_board(&ts).await;
    let path = recorded_file(&ts, &theirs, &board_id).await;
    a_doomed_object_that_is_held_is_kept(&ts, &path).await;
    ts.teardown().await;
}

/// A write of a file to a new board that goes on until the test ends it: what is sent
/// to the returned writer, then its end when it is dropped. Returns the board, the
/// place the file is being written at (marked once the write began), the writer and
/// the write.
async fn slow_write(
    ts: &TestServer,
) -> (
    String,
    String,
    tokio::io::DuplexStream,
    tokio::task::JoinHandle<Result<String, fvoci_server::error::AppError>>,
) {
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let (mut writer, mut reader) = tokio::io::duplex(64);
    let app = ts.app.clone();
    let board_id = board.id.clone();
    let write =
        tokio::spawn(async move { app.save_file(&mut reader, &board_id, "slow.txt").await });
    writer
        .write_all(b"the first bytes")
        .await
        .expect("the write takes them");
    let mut path = None;
    for _ in 0..500 {
        let listed = all_listed(ts).await;
        if let Some(object) = listed.into_iter().find(|o| o.state == LooseState::Writing) {
            path = Some(object.path);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let path = path.expect("the place is marked before it is written");
    (board.id, path, writer, write)
}

/// Ends a slow write and returns what it answered.
async fn finish(
    mut writer: tokio::io::DuplexStream,
    write: tokio::task::JoinHandle<Result<String, fvoci_server::error::AppError>>,
) -> Result<String, fvoci_server::error::AppError> {
    writer
        .write_all(b" and the last")
        .await
        .expect("the write takes them");
    drop(writer);
    write.await.expect("the write does not panic")
}

/// A file whose record cannot be written, as its board went to the trash while it was
/// sent, leaves no object and nothing listed. One the storage fails to write is doomed
/// and removed by the housekeeping once what the storage may have taken is there.
async fn a_file_that_is_not_recorded_leaves_nothing(ts: &TestServer) {
    let (board_id, path, writer, write) = slow_write(ts).await;
    ts.client
        .delete(&format!("{TEST_WS}/containers/{board_id}"))
        .await
        .check_no_content();
    finish(writer, write)
        .await
        .expect_err("the record of a board that went fails");
    assert!(!exists(ts, &path).await, "the object went with the failure");
    assert_eq!(all_listed(ts).await, [], "and nothing is left listed");

    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let faults = ts
        .app
        .files()
        .faults()
        .expect("a test's storage takes faults");
    faults.fail(FaultyOp::Write, 1);
    ts.app
        .save_file(&mut &b"never written"[..], &board.id, "a.txt")
        .await
        .expect_err("the storage fails");
    let listed = all_listed(ts).await;
    assert_eq!(listed.len(), 1, "the place is doomed: {listed:?}");
    let now = now_millis();
    let settled = now + 2 * 60 * 60 * 1000;
    assert_eq!(
        remove_doomed(ts, settled).await,
        RemovedObjects {
            removed: 1,
            failed: 0,
            kept: 0,
        },
        "and removed by the housekeeping, with nothing there"
    );

    // The storage took the object and its answer was lost: the object is left until
    // the housekeeping removes it, by when a late write is there.
    faults.fail(FaultyOp::WriteAnswerLost, 1);
    ts.app
        .save_file(&mut &b"taken, answer lost"[..], &board.id, "b.txt")
        .await
        .expect_err("the storage answers a failure");
    let listed = all_listed(ts).await;
    let [doomed] = listed.as_slice() else {
        panic!("one place is doomed: {listed:?}");
    };
    assert_eq!(doomed.state, LooseState::Doomed, "{doomed:?}");
    assert!(doomed.retry_at > now, "not removed at once: {doomed:?}");
    assert!(exists(ts, &doomed.path).await, "the object is there");
    assert_eq!(
        remove_doomed(ts, now).await,
        RemovedObjects::default(),
        "not before the write has settled"
    );
    assert_eq!(
        remove_doomed(ts, settled).await,
        RemovedObjects {
            removed: 1,
            failed: 0,
            kept: 0,
        },
        "then it goes"
    );
    assert!(!exists(ts, &doomed.path).await, "the object is gone");
}

/// Every listed object, of every organization.
pub(crate) async fn all_listed(ts: &TestServer) -> Vec<fvoci_store::LooseObject> {
    ts.app
        .housekeeping()
        .store()
        .loose_objects()
        .await
        .expect("listed")
}

/// The housekeeping leaves a write that is under way alone, however long it takes
/// within its grace; one that outlasts its grace is doomed, and what it wrote at last
/// is doomed again and removed, with no record.
async fn the_housekeeping_leaves_a_slow_write_alone(ts: &TestServer) {
    let (board_id, path, writer, write) = slow_write(ts).await;
    let later = now_millis() + WRITING_GRACE_MILLIS - 60_000;
    assert_eq!(
        remove_doomed(ts, later).await,
        RemovedObjects::default(),
        "nothing is doomed within the grace"
    );
    let stored_name = finish(writer, write).await.expect("the file is stored");
    assert!(exists(ts, &path).await, "the object is there");
    ts.app
        .store()
        .get_file_record(&board_id, &stored_name)
        .await
        .expect("and recorded");
    assert_eq!(all_listed(ts).await, [], "the record took the mark away");

    let (board_id, path, writer, write) = slow_write(ts).await;
    let past_the_grace = now_millis() + WRITING_GRACE_MILLIS + 60_000;
    remove_doomed(ts, past_the_grace).await;
    finish(writer, write)
        .await
        .expect_err("a write doomed while it went on is not recorded");
    let listed = all_listed(ts).await;
    assert_eq!(
        paths_of(&listed, LooseState::Doomed),
        [path.as_str()],
        "what it wrote is doomed again"
    );
    assert_eq!(
        remove_doomed(ts, past_the_grace).await,
        RemovedObjects {
            removed: 1,
            failed: 0,
            kept: 0,
        },
        "and removed"
    );
    assert!(!exists(ts, &path).await, "the object is gone");
    let files = ts
        .app
        .store()
        .get_file_record(&board_id, &stored_name_at(&path))
        .await;
    assert!(files.is_err(), "nothing is recorded: {files:?}");
}

fn paths_of(objects: &[fvoci_store::LooseObject], state: LooseState) -> Vec<&str> {
    objects
        .iter()
        .filter(|object| object.state == state)
        .map(|object| object.path.as_str())
        .collect()
}

#[tokio::test]
async fn on_the_disk_a_write_is_marked_until_it_is_recorded() {
    let ts = Setup::native().start().await.init_basic().await;
    a_file_that_is_not_recorded_leaves_nothing(&ts).await;
    the_housekeeping_leaves_a_slow_write_alone(&ts).await;
    ts.teardown().await;
}

#[tokio::test]
#[ignore = "needs an S3-compatible server; see tests/storage.rs"]
async fn with_s3_a_write_is_marked_until_it_is_recorded() {
    let ts = s3_server().await;
    a_file_that_is_not_recorded_leaves_nothing(&ts).await;
    the_housekeeping_leaves_a_slow_write_alone(&ts).await;
    ts.teardown().await;
}

/// A file copied with a card is marked while it is copied and recorded with the mark
/// taken away; a copy the storage fails is doomed and removed once it has settled, and
/// the copied block names no file.
#[tokio::test]
async fn a_copied_file_is_marked_until_it_is_recorded() {
    let ts = Setup::native().start().await.init_basic().await;
    let (board_id, path) = uploaded(&ts, b"copied with its card").await;
    let stored_name = stored_name_at(&path);
    let attachment = || fvoci_model::Block {
        id: new_id(),
        container_id: board_id.clone(),
        block_type: fvoci_model::BlockType::Attachment,
        fields: serde_json::json!({ "fileId": stored_name })
            .as_object()
            .expect("an object")
            .clone(),
        ..fvoci_model::Block::default()
    };
    let file_of = |block: &fvoci_model::Block| {
        block.fields["fileId"]
            .as_str()
            .expect("a file name")
            .to_owned()
    };

    let mut copied = [attachment()];
    ts.app.copy_card_files_into(&board_id, &mut copied).await;
    let copy = file_of(&copied[0]);
    assert!(!copy.is_empty() && copy != stored_name, "a copy: {copy:?}");
    let record = ts
        .app
        .store()
        .get_file_record(&board_id, &copy)
        .await
        .expect("the copy is recorded");
    assert!(exists(&ts, &record.path).await, "and there");
    assert_eq!(all_listed(&ts).await, [], "the record took the mark away");

    let faults = ts
        .app
        .files()
        .faults()
        .expect("a test's storage takes faults");
    faults.fail(FaultyOp::Copy, 1);
    let mut failed = [attachment()];
    ts.app.copy_card_files_into(&board_id, &mut failed).await;
    assert_eq!(file_of(&failed[0]), "", "the block names no file");
    let listed = all_listed(&ts).await;
    assert_eq!(
        paths_of(&listed, LooseState::Doomed).len(),
        1,
        "a copy that failed is doomed: {listed:?}"
    );
    assert_eq!(
        remove_doomed(&ts, now_millis() + 2 * 60 * 60 * 1000).await,
        RemovedObjects {
            removed: 1,
            failed: 0,
            kept: 0,
        },
        "and removed once it has settled"
    );
    assert_eq!(all_listed(&ts).await, [], "nothing is left listed");
    ts.teardown().await;
}

/// The metrics count the listed objects by state, and the doomed ones whose removal
/// failed again when it was tried again by the kind of the error, after each run of the
/// housekeeping.
#[tokio::test]
async fn the_metrics_count_the_list_and_the_failing_removals() {
    let ts = Setup::native()
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    uploaded(&ts, b"its removal fails").await;
    ts.app
        .housekeeping()
        .store()
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("collected");
    let faults = ts
        .app
        .files()
        .faults()
        .expect("a test's storage takes faults");
    faults.fail(FaultyOp::Delete, 2);
    let now = now_millis();
    remove_doomed(&ts, now).await;
    let counted = |body: &str| {
        (
            sample(body, "fvoci_loose_objects", &[("state", "doomed")]),
            sample(
                body,
                "fvoci_loose_object_removals_failing",
                &[("error", "io")],
            ),
        )
    };
    let body = scrape(&ts).await;
    assert_eq!(
        counted(&body),
        (Some(1.0), Some(0.0)),
        "failed once, not yet tried again: {body}"
    );
    remove_doomed(&ts, now + 90 * 60 * 1000).await;
    let body = scrape(&ts).await;
    assert_eq!(
        counted(&body),
        (Some(1.0), Some(1.0)),
        "failed again when tried again: {body}"
    );
    remove_doomed(&ts, now + DAY_MILLIS).await;
    let body = scrape(&ts).await;
    assert_eq!(counted(&body), (Some(0.0), Some(0.0)), "{body}");
    ts.teardown().await;
}
