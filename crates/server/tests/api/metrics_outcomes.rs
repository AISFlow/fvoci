//! The metrics of what the server's work came to: the problems the API refused with, the
//! calls of the database, and the operations on the file storage.

use base64::Engine as _;
use fvoci_model::ContainerVisibility;
use reqwest::{Method, StatusCode};
use sha2::Digest as _;

use crate::common::TEST_WS;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TestServer};
use crate::metrics::{sample, scrape};

/// A server that keeps metrics and serves them at a free local port.
async fn server_with_metrics() -> TestServer {
    Setup::native().metrics_at("127.0.0.1:0").start().await
}

/// How many answers were the problem `name`.
fn problems(body: &str, name: &str) -> Option<f64> {
    sample(body, "fvoci_http_problems_total", &[("problem", name)])
}

/// The lines of the samples of `name`.
fn series<'a>(body: &'a str, name: &str) -> Vec<&'a str> {
    body.lines()
        .filter(|line| line.starts_with(name) && line[name.len()..].starts_with(['{', ' ']))
        .collect()
}

/// The values of the label `problem` of the samples of the problems.
fn problem_labels(body: &str) -> Vec<String> {
    series(body, "fvoci_http_problems_total")
        .iter()
        .filter_map(|line| {
            let rest = line.split_once("problem=\"")?.1;
            Some(rest.split_once('"')?.0.to_owned())
        })
        .collect()
}

#[tokio::test]
async fn refusals_are_counted_by_the_name_of_their_problem() {
    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let before = scrape(&server).await;
    assert_eq!(
        problems(&before, "username-taken"),
        Some(0.0),
        "every problem is there from the start: {before}"
    );
    let stranger = server.anonymous();
    for _ in 0..2 {
        let response = stranger.get("/users/me").await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    }
    let response = server.client.get("/no-such-route").await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    // Refused by the framework, and made a problem.
    let response = server
        .client
        .get(&format!("{TEST_WS}/containers/%FF"))
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    let response = server.client.get("/ping").await;
    assert_eq!(response.status, StatusCode::OK);
    let after = scrape(&server).await;
    let counted = |name: &str| {
        problems(&after, name).unwrap_or_default() - problems(&before, name).unwrap_or_default()
    };
    assert_eq!(counted("unauthorized"), 2.0, "{after}");
    assert_eq!(counted("not-found"), 1.0, "{after}");
    assert_eq!(counted("bad-request"), 1.0, "{after}");
    assert_eq!(
        problem_labels(&before),
        problem_labels(&after),
        "the same problems, counted: {after}"
    );
    server.teardown().await;
}

#[tokio::test]
async fn what_a_client_sends_makes_no_problem_of_its_own() {
    let server = server_with_metrics().await;
    let before = scrape(&server).await;
    let mut asked = Vec::new();
    for n in 0..40 {
        let word = fvoci_model::new_id();
        let path = match n % 4 {
            0 => format!("/api/v1/{word}"),
            1 => format!("/api/v1/containers/{word}"),
            2 => format!("/api/v1/problems/{word}"),
            _ => format!("/problems/{word}"),
        };
        let method = if n % 2 == 0 {
            Method::GET
        } else {
            Method::from_bytes(format!("X{n}").as_bytes()).expect("a method")
        };
        Client::send(server.client.raw_request(method, &path)).await;
        asked.push(word);
    }
    let after = scrape(&server).await;
    let labels = problem_labels(&after);
    assert_eq!(labels, problem_labels(&before), "no new problem: {after}");
    for label in &labels {
        assert!(
            !label.contains('/') && label.len() < 40,
            "a name, and nothing asked: {label}"
        );
    }
    for word in &asked {
        assert!(
            !after.contains(word.as_str()),
            "nothing asked is a label: {after}"
        );
    }
    server.teardown().await;
}

#[tokio::test]
async fn the_calls_of_the_database_are_measured() {
    let server = server_with_metrics().await;
    let before = scrape(&server).await;
    let db = server.app.store().db_type().as_str();
    let reads = |body: &str| {
        sample(
            body,
            "fvoci_db_call_duration_seconds_count",
            &[("access", "read"), ("db", db)],
        )
        .unwrap_or_default()
    };
    let waits = |body: &str| {
        sample(body, "fvoci_db_pool_wait_seconds_count", &[("db", db)]).unwrap_or_default()
    };
    let response = server.client.get("/ping").await;
    assert_eq!(response.status, StatusCode::OK);
    let after = scrape(&server).await;
    // The second scrape, within five seconds of the first, reads nothing itself: what
    // it counts is kept that long. So what is counted anew is ping's.
    assert!(reads(&after) >= reads(&before) + 1.0, "ping reads: {after}");
    assert!(
        waits(&after) >= waits(&before) + 1.0,
        "and waits for a connection: {after}"
    );
    for access in ["read", "write"] {
        assert_eq!(
            sample(
                &after,
                "fvoci_db_call_failures_total",
                &[("access", access), ("db", db)]
            ),
            Some(0.0),
            "nothing failed, and the count is there from the start: {after}"
        );
    }
    for line in after.lines().filter(|line| line.starts_with("fvoci_db_")) {
        let db_named = ["db=\"sqlite\"", "db=\"postgres\""]
            .iter()
            .any(|value| line.contains(value));
        assert!(db_named || !line.contains("db="), "{line}");
        if line.contains("access=") {
            assert!(
                line.contains("access=\"read\"") || line.contains("access=\"write\""),
                "{line}"
            );
        }
    }
    server.teardown().await;
}

#[tokio::test]
async fn a_server_without_metrics_gives_its_store_no_observer() {
    let server = Setup::native().start().await;
    assert!(
        server.app.store().observe(Box::new(NoObserver)),
        "the store had none"
    );
    server.teardown().await;
}

/// An observer that is told nothing it keeps.
#[derive(Debug)]
struct NoObserver;

impl fvoci_store::Observer for NoObserver {
    fn called(&self, _: fvoci_store::Call) {}

    fn waited_for_connection(&self, _: fvoci_store::DbType, _: std::time::Duration) {}
}

/// How many operations on the file storage were measured, and how many failed.
fn files_operations(body: &str, backend: &str, operation: &str) -> (f64, f64) {
    let labels = [("backend", backend), ("operation", operation)];
    (
        sample(
            body,
            "fvoci_files_operation_duration_seconds_count",
            &labels,
        )
        .unwrap_or_default(),
        sample(body, "fvoci_files_operation_failures_total", &labels).unwrap_or_default(),
    )
}

#[tokio::test]
async fn the_operations_on_the_local_disk_are_measured() {
    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let board = server
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = crate::files::upload(&server.client, &board.id, "a.txt", b"abc").await;
    let file_id = crate::files::uploaded_id(&response);
    let download = server
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{file_id}",
            board.id
        ))
        .await;
    download.check_ok();
    let missing = server
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/7{}.txt",
            board.id,
            fvoci_model::new_id()
        ))
        .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    let body = scrape(&server).await;
    let (puts, put_failures) = files_operations(&body, "local", "put");
    assert!(puts >= 1.0, "the upload is stored: {body}");
    assert_eq!(put_failures, 0.0, "{body}");
    let (gets, get_failures) = files_operations(&body, "local", "get");
    assert!(gets >= 1.0, "the download is read: {body}");
    assert_eq!(
        get_failures, 0.0,
        "a file that is not there is no failure: {body}"
    );
    assert!(
        series(&body, "fvoci_files_operation_duration_seconds_count")
            .iter()
            .all(|line| line.contains("backend=\"local\"")),
        "one backend: {body}"
    );
    server.teardown().await;
}

#[tokio::test]
async fn an_upload_past_its_limit_is_no_failure_of_the_storage() {
    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .configure(|config| config.max_file_bytes = 1)
        .start()
        .await
        .init_basic()
        .await;
    let board = server
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = crate::files::upload(&server.client, &board.id, "a.txt", b"test").await;
    response.check_request_entity_too_large();
    let body = scrape(&server).await;
    let (puts, failures) = files_operations(&body, "local", "put");
    assert_eq!(failures, 0.0, "the limit is the request's: {body}");
    assert!(puts <= 1.0, "measured once at most, as given up: {body}");
    server.teardown().await;
}

/// With S3, every step of an upload in parts is measured on the object store.
#[tokio::test]
#[ignore = "needs an S3-compatible server; see tests/storage.rs"]
async fn with_s3_the_steps_of_an_upload_in_parts_are_measured() {
    assert!(
        std::env::var("FVOCI_S3_TEST_ENDPOINT").is_ok()
            && std::env::var("FVOCI_S3_TEST_BUCKET").is_ok(),
        "set FVOCI_S3_TEST_ENDPOINT and FVOCI_S3_TEST_BUCKET to run this test"
    );
    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .configure(|config| {
            let bucket = std::env::var("FVOCI_S3_TEST_BUCKET").unwrap_or_default();
            crate::uploads::configure_s3(config, &bucket);
        })
        .start()
        .await
        .init_basic()
        .await;
    let board = server
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let open = |name: &'static str| {
        let client = &server.client;
        let board_id = board.id.clone();
        async move {
            let response = client
                .post(
                    &format!("{TEST_WS}/containers/{board_id}/uploads"),
                    &serde_json::json!({"name": name, "size": 3}),
                )
                .await;
            response.check_created();
            response.value()["uploadId"]
                .as_str()
                .expect("an upload has an identifier")
                .to_owned()
        }
    };
    let send_part = async |upload_id: &str| {
        let bytes = b"abc";
        let digest = format!(
            "sha-256=:{}:",
            base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(bytes))
        );
        let request = server
            .client
            .request(
                Method::PUT,
                &format!("{TEST_WS}/uploads/{upload_id}/parts/1"),
            )
            .header("Content-Type", "application/octet-stream")
            .header("Content-Digest", digest)
            .body(bytes.to_vec());
        Client::send(request).await.check_no_content();
    };
    let completed = open("a.txt").await;
    send_part(&completed).await;
    server
        .client
        .post_empty(&format!("{TEST_WS}/uploads/{completed}/complete"))
        .await
        .check_created();
    let abandoned = open("b.txt").await;
    send_part(&abandoned).await;
    server
        .client
        .delete(&format!("{TEST_WS}/uploads/{abandoned}"))
        .await
        .check_no_content();
    let body = scrape(&server).await;
    for operation in [
        "multipart_create",
        "multipart_part",
        "multipart_complete",
        "multipart_abort",
    ] {
        let (operations, failures) = files_operations(&body, "s3", operation);
        assert!(operations >= 1.0, "{operation}: {body}");
        assert_eq!(failures, 0.0, "{operation}: {body}");
    }
    server.teardown().await;
}
