//! A real multipart completion overlapped with another process's upload cleaner: the
//! second process does not hold the lock of the server, and its cleaner does not run;
//! the server's own cleaner waits for the session's turn.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use bytes::Bytes;
use fvoci_model::{ContainerVisibility, new_id};
use fvoci_server::app::{App, CLOSED_UPLOAD_GRACE_MILLIS, FileToUpload};
use fvoci_server::auth::now_millis;
use fvoci_server::config::{Configuration, S3Config};
use fvoci_store::{Call, DbType, Observer, Store};
use sha2::{Digest as _, Sha256};
use tokio::sync::oneshot;

use crate::common::{Setup, TEST_WORKSPACE_ID};

fn configure(config: &mut Configuration) {
    let var = |key: &str, default: &str| std::env::var(key).unwrap_or_else(|_| default.to_owned());
    "s3".clone_into(&mut config.file_storage);
    config.file_storage_s3 = S3Config {
        endpoint: var("FVOCI_S3_TEST_ENDPOINT", ""),
        bucket: var("FVOCI_S3_TEST_BUCKET", ""),
        access_key_id: var("FVOCI_S3_TEST_ACCESS_KEY", "any"),
        secret_access_key: var("FVOCI_S3_TEST_SECRET_KEY", "any"),
        region: var("FVOCI_S3_TEST_REGION", "us-east-1"),
        https: var("FVOCI_S3_TEST_SSL", "false") == "true",
        key_prefix: format!("file-review/{}", new_id()),
        ..S3Config::default()
    };
}

#[derive(Debug)]
struct BeforeFileRecord {
    connections: AtomicUsize,
    entered: Mutex<Option<oneshot::Sender<()>>>,
    resume: Mutex<mpsc::Receiver<()>>,
}

impl Observer for BeforeFileRecord {
    fn called(&self, _: Call) {}

    fn waited_for_connection(&self, _: DbType, _: Duration) {
        // The first three calls read the parts, close the session, and reread parts.
        // The fourth records the file, after S3 has made the completed object.
        if self.connections.fetch_add(1, Ordering::SeqCst) != 3 {
            return;
        }
        if let Some(entered) = self.entered.lock().expect("gate lock").take() {
            let _ = entered.send(());
        }
        self.resume
            .lock()
            .expect("gate lock")
            .recv_timeout(Duration::from_secs(30))
            .expect("the cleaner releases the completion");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs FVOCI_S3_TEST_ENDPOINT and FVOCI_S3_TEST_BUCKET"]
async fn with_s3_a_completion_cannot_record_an_object_another_cleaner_removed() {
    let ts = Setup::native()
        .configure(configure)
        .start()
        .await
        .init_basic()
        .await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let user_id = ts.client.me().await.id;
    let bytes = Bytes::from_static(b"multipart bytes");
    let upload = ts
        .app
        .create_upload(
            &user_id,
            &board.id,
            FileToUpload {
                name: "multipart.txt".to_owned(),
                size: i64::try_from(bytes.len()).expect("small file"),
                content_type: String::new(),
            },
        )
        .await
        .expect("a real multipart session");
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    ts.app
        .put_upload_part(
            &upload,
            1,
            u64::try_from(bytes.len()).expect("small file"),
            &digest,
            futures_util::stream::iter([Ok::<_, std::io::Error>(bytes)]),
        )
        .await
        .expect("the real part is stored");

    // The completion of the server pauses before its file is recorded.
    let (entered, reached) = oneshot::channel();
    let (resume, wait) = mpsc::channel();
    assert!(ts.app.store().observe(Box::new(BeforeFileRecord {
        connections: AtomicUsize::new(0),
        entered: Mutex::new(Some(entered)),
        resume: Mutex::new(wait),
    })));
    let completing = ts.app.clone();
    let completing_upload = upload.clone();
    let runtime = tokio::runtime::Handle::current();
    let completion = tokio::task::spawn_blocking(move || {
        runtime.block_on(async move { completing.complete_upload(&completing_upload).await })
    });
    tokio::time::timeout(Duration::from_secs(20), reached)
        .await
        .expect("completion reaches its record")
        .expect("the gate is reached");
    assert!(ts.app.files().exists(&upload.path).await.expect("S3 read"));

    // A second process at the same database, with the closed-session grace elapsed:
    // without the lock of the server its cleaner does not run.
    let independent = Store::connect(
        ts.app.store().db_type(),
        &ts.app.config().database_connection,
    )
    .await
    .expect("an independent pool");
    let second = App::new(
        ts.app.settings().clone(),
        independent.clone(),
        ts.app.files().clone(),
    )
    .expect("an independent application");
    let refused = second
        .housekeeping()
        .clean_up_uploads(now_millis() + CLOSED_UPLOAD_GRACE_MILLIS + 1_000)
        .await
        .expect_err("the second application's cleaner does not run");
    assert_eq!(refused.status(), 503, "{refused:?}");
    assert!(ts.app.files().exists(&upload.path).await.expect("S3 read"));
    // The server's own cleaner, at the same moment, takes the session's turn after the
    // completion, however the two meet.
    let cleaner = ts.app.housekeeping();
    let cleaning = tokio::spawn(async move {
        cleaner
            .clean_up_uploads(now_millis() + CLOSED_UPLOAD_GRACE_MILLIS + 1_000)
            .await
    });
    resume.send(()).expect("the record may proceed");
    let completed = completion.await.expect("completion returns");
    cleaning
        .await
        .expect("the cleaner returns")
        .expect("the server's cleaner runs");
    let still_present = ts.app.files().exists(&upload.path).await.expect("S3 read");
    independent.close().await;
    ts.teardown().await;
    assert!(
        completed.is_err() || still_present,
        "a successful multipart completion needs its object: {completed:?}"
    );
    assert!(completed.is_ok(), "{completed:?}");
}
