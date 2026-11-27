//! Whether the server is alive (`ping`) and ready (`ready`): the database answers and
//! the file storage can be used. A failing check says which, and never why.

use futures_util::TryStreamExt as _;
use reqwest::StatusCode;
use serde_json::json;

use crate::common::{Setup, TestServer};
use crate::metrics::{sample, scrape};

/// What `ready` answers to someone without a session.
async fn ready(server: &TestServer) -> crate::common::ApiResponse {
    server.anonymous().get("/ready").await
}

#[tokio::test]
async fn a_server_that_can_serve_is_ready_and_says_so_without_a_session() {
    let server = Setup::native().metrics_at("127.0.0.1:0").start().await;
    let response = ready(&server).await;
    response.check_ok();
    assert_eq!(response.value(), json!({"database": "ok", "files": "ok"}));
    let body = scrape(&server).await;
    assert_eq!(sample(&body, "fvoci_ready", &[]), Some(1.0), "{body}");
    for check in ["database", "files"] {
        assert_eq!(
            sample(&body, "fvoci_ready_check", &[("check", check)]),
            Some(1.0),
            "{check}: {body}"
        );
    }
    assert!(
        !body.contains("fvoci_files_operation_duration_seconds_count"),
        "the check of the files is no operation on them: {body}"
    );
    server.teardown().await;
}

#[tokio::test]
async fn without_the_database_the_server_is_alive_and_not_ready_and_says_no_more() {
    let server = Setup::native().metrics_at("127.0.0.1:0").start().await;
    server.app.store().close().await;
    let alive = server.anonymous().get("/ping").await;
    alive.check_ok();
    assert_eq!(
        alive.value()["version"],
        json!(env!("CARGO_PKG_VERSION")),
        "ping answers even when the database is closed"
    );

    let response = ready(&server).await;
    response.check_error(StatusCode::SERVICE_UNAVAILABLE, "not ready: database");
    let text = response.text().to_lowercase();
    for cause in [
        "pool",
        "closed",
        "sqlx",
        "sqlite",
        "postgres",
        "error",
        "127.0.0.1",
    ] {
        assert!(!text.contains(cause), "no cause in the answer: {text}");
    }
    let body = scrape(&server).await;
    assert_eq!(sample(&body, "fvoci_ready", &[]), Some(0.0), "{body}");
    assert_eq!(
        sample(&body, "fvoci_ready_check", &[("check", "database")]),
        Some(0.0),
        "{body}"
    );
    assert_eq!(
        sample(&body, "fvoci_ready_check", &[("check", "files")]),
        Some(1.0),
        "{body}"
    );
    server.teardown().await;
}

/// A database that answers but has lost a table every signed-in request reads is not
/// ready: that it answers a statement says nothing of its schema.
#[tokio::test]
async fn readiness_refuses_a_database_missing_a_required_table() {
    let server = Setup::native().start().await;
    server
        .app
        .store()
        .execute_unchecked("DROP TABLE sessions")
        .await
        .expect("the test removes a required table");
    assert!(
        server.app.store().count_live_sessions(1, 0).await.is_err(),
        "session operations cannot run with the incomplete schema"
    );
    // No readiness result has been kept yet, so a stale one cannot explain the answer.
    let response = ready(&server).await;
    response.check_error(StatusCode::SERVICE_UNAVAILABLE, "not ready: database");
    assert!(
        !response.text().contains("sessions"),
        "no table in the answer: {}",
        response.text()
    );
    server.teardown().await;
}

#[tokio::test]
async fn a_files_directory_that_is_no_directory_is_not_ready() {
    let server = Setup::native()
        .configure(|config| {
            // A file, which the server only reads here.
            concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")
                .clone_into(&mut config.file_storage_path);
        })
        .start()
        .await;
    let response = ready(&server).await;
    response.check_error(StatusCode::SERVICE_UNAVAILABLE, "not ready: files");
    assert!(
        !response.text().contains("Cargo.toml"),
        "no path in the answer: {}",
        response.text()
    );
    server.teardown().await;
}

/// A directory the server may not write in is not ready: that it is a directory says
/// nothing of whether a file can be stored. A check that fails leaves no partial file behind.
#[cfg(unix)]
#[tokio::test]
async fn readiness_refuses_a_files_directory_that_cannot_be_written() {
    use std::os::unix::fs::PermissionsExt as _;

    let server = Setup::native().start().await;
    let directory = std::path::Path::new(&server.app.config().file_storage_path);
    std::fs::create_dir_all(directory).expect("the test's directory exists");
    let permissions = std::fs::metadata(directory)
        .expect("the original permissions")
        .permissions();
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o555))
        .expect("the test removes write permission");
    let mut bytes: &[u8] = b"probe";
    let write = server
        .app
        .files()
        .write("readiness-probe.txt", &mut bytes)
        .await;
    let response = ready(&server).await;
    std::fs::set_permissions(directory, permissions).expect("restore before cleanup");
    assert!(
        write.is_err(),
        "run as an unprivileged user so the write fails"
    );
    response.check_error(StatusCode::SERVICE_UNAVAILABLE, "not ready: files");
    assert_nothing_left(directory);
    assert!(
        !response
            .text()
            .contains(&server.app.config().file_storage_path),
        "no path in the answer: {}",
        response.text()
    );
    server.teardown().await;
}

/// The check left neither its file nor a partial one in `directory`.
fn assert_nothing_left(directory: &std::path::Path) {
    assert!(
        !directory.join(".fvoci-ready").exists(),
        "the file of the check"
    );
    let writing = directory.join(fvoci_server::storage::WRITING_DIR);
    let partial: Vec<_> = std::fs::read_dir(&writing)
        .map(|entries| {
            entries
                .map(|entry| entry.expect("an entry").path())
                .collect()
        })
        .unwrap_or_default();
    assert!(partial.is_empty(), "nothing being written: {partial:?}");
}

#[tokio::test]
async fn the_check_of_the_files_leaves_nothing_in_the_directory() {
    let server = Setup::native().start().await;
    ready(&server).await.check_ok();
    assert_nothing_left(std::path::Path::new(&server.app.config().file_storage_path));
    server.teardown().await;
}

#[tokio::test]
async fn the_checks_are_kept_for_a_few_seconds() {
    let server = Setup::native().start().await;
    ready(&server).await.check_ok();
    server.app.store().close().await;
    ready(&server).await.check_ok();
    server.teardown().await;
}

/// With S3, a bucket that takes, gives and removes a file is ready, and leaves nothing
/// behind; one that is not there is not ready.
#[tokio::test]
#[ignore = "needs an S3-compatible server; see tests/storage.rs"]
async fn with_s3_a_bucket_that_is_not_there_is_not_ready() {
    assert!(
        std::env::var("FVOCI_S3_TEST_ENDPOINT").is_ok()
            && std::env::var("FVOCI_S3_TEST_BUCKET").is_ok(),
        "set FVOCI_S3_TEST_ENDPOINT and FVOCI_S3_TEST_BUCKET to run this test"
    );
    let server = Setup::native()
        .configure(|config| {
            let bucket = std::env::var("FVOCI_S3_TEST_BUCKET").unwrap_or_default();
            crate::uploads::configure_s3(config, &bucket);
        })
        .start()
        .await;
    ready(&server).await.check_ok();
    let left: Vec<_> = server
        .app
        .files()
        .list()
        .try_collect::<Vec<_>>()
        .await
        .expect("the bucket lists")
        .into_iter()
        .filter(|object| object.path.ends_with(".fvoci-ready"))
        .collect();
    assert!(left.is_empty(), "the check leaves no object: {left:?}");
    server.teardown().await;

    let server = Setup::native()
        .configure(|config| crate::uploads::configure_s3(config, "fvoci-no-such-bucket"))
        .start()
        .await;
    let response = ready(&server).await;
    response.check_error(StatusCode::SERVICE_UNAVAILABLE, "not ready: files");
    assert!(
        !response.text().contains("no-such-bucket"),
        "no bucket in the answer: {}",
        response.text()
    );
    server.teardown().await;
}
