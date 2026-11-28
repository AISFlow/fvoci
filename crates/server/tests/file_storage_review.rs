//! Independent failure cases for the boundary between object storage and its records.

#[path = "api/common.rs"]
mod common;

#[path = "file_storage_review/lock_handover.rs"]
mod lock_handover;

#[path = "file_storage_review/races.rs"]
mod races;

#[path = "file_storage_review/s3.rs"]
mod s3;

#[path = "file_storage_review/startup.rs"]
mod startup;

use fvoci_model::{Body, Card, ContainerVisibility, FileRecord, Upload, UploadPurpose};
use fvoci_server::app::ChangeNotification;
use fvoci_server::auth::now_millis;
use fvoci_server::auth::secret::new_secret;
use fvoci_server::storage::StorageError;
use fvoci_store::StoreError;

use common::{Setup, TEST_WORKSPACE_ID, TestServer};

async fn recorded(ts: &TestServer) -> FileRecord {
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let name = ts
        .app
        .save_file(&mut &b"keep these bytes"[..], &board.id, "kept.txt")
        .await
        .expect("the file is stored");
    ts.app
        .store()
        .get_file_record(&board.id, &name)
        .await
        .expect("the file is recorded")
}

/// This is the same malformed-session case as the existing misbound-upload tests,
/// except that its path has an empty segment. Both storage backends accepted this
/// spelling of the recorded object's place, while `held_objects` compares raw text.
#[tokio::test]
async fn abandoning_an_upload_with_a_path_alias_preserves_the_recorded_object() {
    let ts = Setup::native().start().await.init_basic().await;
    let file = recorded(&ts).await;
    let upload = Upload {
        id: new_secret(),
        user_id: ts.client.me().await.id,
        purpose: UploadPurpose::File,
        container_id: file.container_id.clone(),
        name: "misbound.txt".to_owned(),
        size: 16,
        part_size: 16,
        created_at: now_millis(),
        expires_at: now_millis() + 60_000,
        multipart_id: "left-over-multipart".to_owned(),
        path: file.path.replacen('/', "//", 1),
        ..Upload::default()
    };
    // The store takes a place spelled the one way only; the session is stored at the
    // record's place and given the other spelling past the store, as a row stored
    // before that check would have it.
    let refused = ts
        .app
        .store()
        .create_upload(&upload)
        .await
        .expect_err("another spelling of a place");
    assert!(matches!(refused, StoreError::InvalidPath(_)), "{refused:?}");
    ts.app
        .store()
        .create_upload(&Upload {
            path: file.path.clone(),
            ..upload.clone()
        })
        .await
        .expect("the misbound session is stored");
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE uploads SET path = '{}' WHERE id = '{}'",
            upload.path, upload.id
        ))
        .await
        .expect("the session names the place the other way");
    assert!(
        matches!(
            ts.app.files().size(&upload.path).await,
            Err(StorageError::InvalidPath(_))
        ),
        "the storage takes no other spelling of a place"
    );
    ts.app.abandon_upload(&upload).await.expect("abandoned");
    let still_recorded = ts
        .app
        .store()
        .get_file_record(&file.container_id, &file.stored_name)
        .await
        .is_ok();
    let still_present = ts.app.files().exists(&file.path).await.expect("read");
    ts.teardown().await;
    assert!(still_recorded, "the live record remains");
    assert!(
        still_present,
        "abandonment must preserve its live object's bytes"
    );
}

/// The HTTP handlers `check_body_files` before `create_card` starts its transaction.
/// Run those actual application steps with the collector in the intervening gap:
/// a request that was valid when checked must not commit a dangling image.
#[tokio::test]
async fn a_body_checked_before_collection_cannot_commit_a_missing_image() {
    let ts = Setup::native().start().await.init_basic().await;
    let user_id = ts.client.me().await.id;
    let file = recorded(&ts).await;
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE files SET unreferenced_at = 1 WHERE id = '{}'",
            file.id
        ))
        .await
        .expect("the unreferenced file has already outlived its grace period");
    let body: Body = serde_json::from_value(serde_json::json!({
        "type": "doc", "content": [{
            "type": "image", "attrs": {"fileId": file.stored_name}
        }]
    }))
    .expect("a valid image body");
    ts.app
        .check_body_files(&file.container_id, Some(&body))
        .await
        .expect("the request's image is valid before collection");
    // The normal housekeeping deadline, for a file already eligible when checked.
    let before = now_millis().saturating_sub(ts.app.config().trash_retention_millis());
    let collected = ts
        .app
        .housekeeping()
        .store()
        .collect_unreferenced_files(before)
        .await
        .expect("collected");
    assert_eq!(collected.len(), 1);
    ts.app
        .housekeeping()
        .remove_doomed_objects(now_millis())
        .await
        .expect("the collected object's bytes are removed");
    let created = ts
        .app
        .create_card_unchecked(
            Card {
                body: Some(body.clone()),
                ..Card::default()
            },
            &file.container_id,
            &user_id,
            ChangeNotification::default(),
        )
        .await;
    let still_present = ts.app.files().exists(&file.path).await.expect("read");
    // The refusal is the one the handlers' check gives once the file is gone.
    let checked_now = ts
        .app
        .check_body_files(&file.container_id, Some(&body))
        .await
        .expect_err("the image's file is gone");
    ts.teardown().await;
    assert!(
        created.is_err() || still_present,
        "a successful card creation must retain the validated image: {created:?}"
    );
    if let Err(refused) = created {
        assert_eq!(refused.body(), checked_now.body(), "{refused:?}");
    }
}

/// A mark left by a purge's old snapshot is not permission to delete a file a
/// committed body shows. Exercise the real server's locked deletion of objects too.
#[tokio::test]
async fn a_purge_mark_cannot_doom_the_bytes_of_a_newly_shown_image() {
    let ts = Setup::native().start().await.init_basic().await;
    let file = recorded(&ts).await;
    let body: Body = serde_json::from_value(serde_json::json!({
        "type": "doc", "content": [{
            "type": "image", "attrs": {"fileId": file.stored_name}
        }]
    }))
    .expect("a valid image body");
    ts.app
        .create_card_unchecked(
            Card {
                body: Some(body),
                ..Card::default()
            },
            &file.container_id,
            &ts.client.me().await.id,
            ChangeNotification::default(),
        )
        .await
        .expect("a committed card shows the image");
    // The deterministic store race test reaches this state through purge_trash.
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE files SET unreferenced_at = 1 WHERE id = '{}'",
            file.id,
        ))
        .await
        .expect("the old snapshot's mark");
    assert!(
        ts.app.holds_server_lock(),
        "the real housekeeping worker owns the lock"
    );
    let collected = ts
        .app
        .housekeeping()
        .store()
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("collected");
    ts.app
        .housekeeping()
        .remove_doomed_objects(now_millis())
        .await
        .expect("objects removed");
    let recorded = ts
        .app
        .store()
        .get_file_record(&file.container_id, &file.stored_name)
        .await;
    let present = ts.app.files().exists(&file.path).await.expect("read");
    ts.teardown().await;
    assert!(
        collected.is_empty(),
        "the live reference wins over a stale mark"
    );
    assert!(recorded.is_ok(), "the live file record remains");
    assert!(present, "the committed body's image bytes remain");
}
