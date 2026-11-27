//! A metrics token of fewer than 32 characters keeps the server from starting, and
//! neither a refused token nor a taken one is in the error or the log.
//!
//! A binary of its own: the log is caught by a subscriber of this thread, and the
//! interest of `tracing` in each place that logs is shared by the whole process, so
//! that tests on other threads, without a subscriber, could leave it out.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fvoci_model::new_id;
use fvoci_server::Server;
use fvoci_server::config::Configuration;
use fvoci_store::testing::TestDatabase;

/// 31 and 32 characters.
const SHORT: &str = "short-token-of-31-characters-xx";
const ENOUGH: &str = "token-of-exactly-32-characters-x";

/// What a subscriber of the log writes.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("lock poisoned")).into_owned()
    }
}

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("lock poisoned")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The configuration of a server on `database`, its files under `directory`, keeping
/// metrics with `token`.
fn configuration(
    database: &TestDatabase,
    directory: &std::path::Path,
    token: &str,
) -> Configuration {
    Configuration {
        database_kind: database.db_type().as_str().to_owned(),
        database_connection: database.database_connection().to_owned(),
        file_storage: "local".to_owned(),
        file_storage_path: directory.join("uploads").display().to_string(),
        skip_template_init: true,
        metrics_address: "127.0.0.1:0".to_owned(),
        metrics_token: token.to_owned(),
        ..Configuration::default()
    }
}

#[tokio::test]
async fn a_token_of_fewer_than_thirty_two_characters_keeps_the_server_from_starting() {
    assert_eq!((SHORT.len(), ENOUGH.len()), (31, 32));
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);

    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("metrics-token-log")
        .join(new_id());
    std::fs::create_dir_all(&directory).expect("the directory is made");
    let database = TestDatabase::create(&directory).await;

    let Err(error) = Server::new(configuration(&database, &directory, SHORT), "").await else {
        panic!("the server does not start");
    };
    let error = format!("{error:#}");
    assert!(
        error.contains("metrics_token must be at least 32 characters"),
        "{error}"
    );
    assert!(!error.contains(SHORT), "{error}");

    let server = Server::new(configuration(&database, &directory, ENOUGH), "")
        .await
        .expect("the server starts");
    let metrics = server.metrics_address().expect("the server keeps metrics");
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("a local port is free");
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(server.serve(listener, async move {
        let _ = stopped.await;
    }));
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("an HTTP client")
        .get(format!("http://{metrics}/metrics"))
        .header("authorization", format!("Bearer {ENOUGH}"))
        .send()
        .await
        .expect("an answer");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let _ = stop.send(());
    serving
        .await
        .expect("the server's task ends")
        .expect("the server stops");
    database.remove().await;
    let _ = std::fs::remove_dir_all(&directory);

    let log = captured.text();
    assert!(
        log.contains("metrics server started"),
        "the log was caught: {log}"
    );
    for token in [SHORT, ENOUGH] {
        assert!(!log.contains(token), "the token is not logged: {log}");
    }
}
