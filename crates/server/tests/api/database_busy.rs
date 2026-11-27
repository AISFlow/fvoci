//! An SQLite file that another writer holds: a request that reads is answered all the
//! same, and one that must write and cannot in its time is told to come again (503,
//! `database_busy`) rather than that the server failed (500).

use std::path::PathBuf;

use fvoci_model::{Card, ContainerVisibility};
use fvoci_store::DbType;
use fvoci_store::testing::OtherWriter;
use reqwest::StatusCode;
use serde_json::json;

use crate::common::{Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};

/// The file of the test server's database.
fn database_of(th: &TestServer) -> PathBuf {
    let config = th.app.config();
    let file = config.database_connection.split('?').next();
    PathBuf::from(file.expect("a path"))
}

/// A server on an SQLite file whose statements wait two seconds for another writer
/// (long enough for the server's own writers to take turns on a busy machine, short
/// enough to wait out once), with a card on a board. `None` where the tests run against PostgreSQL.
async fn with_a_card() -> Option<(TestServer, String, Card)> {
    if fvoci_store::testing::db_type_of_the_tests() != DbType::Sqlite {
        return None;
    }
    let th = Setup::native()
        .configure(|config| {
            let file = config
                .database_connection
                .split('?')
                .next()
                .expect("a path");
            config.database_connection = format!("{file}?busy_timeout_ms=2000");
        })
        .start()
        .await
        .init_basic()
        .await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let cards = format!("{TEST_WS}/boards/{}/cards", board.id);
    let card: Card = th
        .client
        .post(&cards, &json!({"title": "Written before"}))
        .await
        .created_json();
    Some((th, cards, card))
}

/// With the rollback journal such a read waited for the writer, and was answered 500
/// when the busy timeout was over ("database is locked").
#[tokio::test]
async fn a_request_that_reads_is_answered_while_another_writer_holds_the_file() {
    let Some((th, _, card)) = with_a_card().await else {
        return;
    };
    let writer = OtherWriter::hold(&database_of(&th)).await;

    let read: Card = th
        .client
        .get(&format!("{TEST_WS}/cards/{}", card.id))
        .await
        .ok_json();
    assert_eq!(read.title, "Written before");
    assert!(!th.client.me().await.id.is_empty(), "who asks is read too");

    writer.let_go().await;
}

/// Writers take turns, with the log too. One that waited its whole time is told that
/// the server cannot for now, with the reason, and the same request is done once the
/// file is free.
#[tokio::test]
async fn a_request_that_cannot_write_in_time_is_told_to_come_again() {
    let Some((th, cards, _)) = with_a_card().await else {
        return;
    };
    let writer = OtherWriter::hold(&database_of(&th)).await;

    let refused = th.client.post(&cards, &json!({"title": "Meanwhile"})).await;
    let (status, problem) = refused.problem();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{problem}");
    assert_eq!(problem["type"], "/problems/database-busy", "{problem}");
    assert_eq!(problem["reason"], "database_busy", "{problem}");

    writer.let_go().await;
    let made: Card = th
        .client
        .post(&cards, &json!({"title": "Meanwhile"}))
        .await
        .created_json();
    assert_eq!(made.title, "Meanwhile");
    let all: Vec<Card> = th.client.get(&cards).await.ok_json();
    let titles: Vec<&str> = all.iter().map(|card| card.title.as_str()).collect();
    assert_eq!(
        titles.iter().filter(|title| **title == "Meanwhile").count(),
        1
    );
}
