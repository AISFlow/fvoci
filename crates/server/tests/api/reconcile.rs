//! The comparison of the file storage with the records: what it reports, and that it
//! dooms only when asked, only what nothing names, and only what is old enough.

use fvoci_model::{Upload, UploadPurpose, new_id};
use fvoci_server::app::{
    Found, Reconciliation, RemovedObjects, UNRECORDED_MIN_AGE_MILLIS, Unrecorded,
};
use fvoci_server::auth::now_millis;
use fvoci_server::auth::secret::new_secret;
use fvoci_server::config::Configuration;

use crate::common::TEST_WS;
use crate::common::{Setup, TestServer};
use crate::files::key_of;
use crate::loose_objects::{all_listed, exists, remove_doomed, uploaded};

/// Compares the storage with the records at `now`.
async fn reconcile(ts: &TestServer, unrecorded: Unrecorded, now: i64) -> Reconciliation {
    ts.app
        .housekeeping()
        .reconcile_files(unrecorded, now)
        .await
        .expect("the comparison runs")
}

/// A recorded file, one kept with its board in the trash, one being written, one an
/// upload session makes, an object nothing names, one not of the server's making and a
/// record whose object is gone: the comparison reports the last three, changes nothing
/// unless asked, dooms only what nothing names at a place of the server's making and
/// only once it is old enough, and the housekeeping removes that alone.
async fn the_comparison_reports_and_dooms_only_when_asked(ts: &TestServer) {
    let (board_id, recorded) = uploaded(ts, b"recorded").await;
    let (trashed_board, trashed) = uploaded(ts, b"in the trash").await;
    ts.client
        .delete(&format!("{TEST_WS}/containers/{trashed_board}"))
        .await
        .check_no_content();
    let (_, missing) = uploaded(ts, b"its object goes").await;
    ts.app.files().delete(&missing).await.expect("removed");
    let writing = key_of(ts, &board_id, "being-written").await;
    ts.app
        .store()
        .mark_writing(&writing, i64::MAX)
        .await
        .expect("marked");
    let stray = key_of(ts, &board_id, "stray").await;
    let uploading = key_of(ts, &board_id, "uploading").await;
    ts.app
        .store()
        .create_upload(&Upload {
            id: new_secret(),
            user_id: ts.client.me().await.id,
            purpose: UploadPurpose::File,
            container_id: board_id.clone(),
            name: "uploading.txt".to_owned(),
            size: 5,
            part_size: 5,
            created_at: now_millis(),
            expires_at: now_millis() + 60_000,
            path: uploading.clone(),
            ..Upload::default()
        })
        .await
        .expect("a session makes its file there");
    // Not of the server's making, as another application's in the same bucket.
    let foreign = "another-application/old.txt".to_owned();
    for path in [&writing, &stray, &uploading, &foreign] {
        ts.app
            .files()
            .write(path, &mut &b"bytes"[..])
            .await
            .expect("written");
    }

    let later = now_millis() + UNRECORDED_MIN_AGE_MILLIS + 60_000;
    let expected = Reconciliation {
        objects: 6,
        unrecorded: Found {
            count: 1,
            paths: vec![stray.clone()],
        },
        foreign: Found {
            count: 1,
            paths: vec![foreign.clone()],
        },
        doomed: 0,
        records: 3,
        missing: Found {
            count: 1,
            paths: vec![missing.clone()],
        },
    };
    assert_eq!(
        reconcile(ts, Unrecorded::Report, later).await,
        expected,
        "the report"
    );
    assert_eq!(all_listed(ts).await.len(), 1, "the report changes nothing");
    assert!(exists(ts, &stray).await, "nor removes anything");

    assert_eq!(
        reconcile(ts, Unrecorded::Doom, now_millis()).await.doomed,
        0,
        "an object written just now is not doomed"
    );
    assert_eq!(
        reconcile(ts, Unrecorded::Doom, later).await,
        Reconciliation {
            doomed: 1,
            ..expected
        },
        "one old enough is"
    );
    assert_eq!(
        reconcile(ts, Unrecorded::Doom, later).await.doomed,
        0,
        "once"
    );
    assert_eq!(
        remove_doomed(ts, later).await,
        RemovedObjects {
            removed: 1,
            failed: 0,
            kept: 0,
        },
        "the housekeeping removes it"
    );
    assert!(!exists(ts, &stray).await, "it is gone");
    for path in [&recorded, &trashed, &writing, &uploading, &foreign] {
        assert!(exists(ts, path).await, "{path} is kept");
    }
}

#[tokio::test]
async fn on_the_disk_the_comparison_reports_and_dooms_only_when_asked() {
    let ts = Setup::native().start().await.init_basic().await;
    the_comparison_reports_and_dooms_only_when_asked(&ts).await;
    ts.teardown().await;
}

/// A bucket of its own: the comparison counts every object under the prefix.
fn own_bucket(config: &mut Configuration) {
    let bucket = std::env::var("FVOCI_S3_TEST_BUCKET").unwrap_or_default();
    crate::uploads::configure_s3(config, &bucket);
    config.file_storage_s3.key_prefix = format!("reconcile-tests/{}", new_id());
}

#[tokio::test]
#[ignore = "needs an S3-compatible server; see tests/storage.rs"]
async fn with_s3_the_comparison_reports_and_dooms_only_when_asked() {
    let ts = Setup::native()
        .configure(own_bucket)
        .start()
        .await
        .init_basic()
        .await;
    the_comparison_reports_and_dooms_only_when_asked(&ts).await;
    ts.teardown().await;
}
