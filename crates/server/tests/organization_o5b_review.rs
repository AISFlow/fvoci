//! Independent checks of the privacy and workspace promises of O5b.

#[path = "api/common.rs"]
mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fvoci_model::{Container, ContainerVisibility};
use fvoci_store::{Access, Call, DbType, Observer};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use common::{HIDDEN_WORKSPACE_ID, ROOT_WS, Setup, TEST_WS, UNKNOWN_ID};

/// Removing a property must not turn its previously redacted view data public.
#[tokio::test]
async fn shared_views_hide_people_after_their_property_is_deleted() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let person = ts.client.me().await.id;
    let board: Container = ts
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({
                "visibility": "private", "title": "Shared plan",
                "cardProperties": [{"id": "owner", "name": "Owner", "type": "person"}],
            }),
        )
        .await
        .created_json();
    ts.client
        .post(
            &format!("{TEST_WS}/boards/{}/views", board.id),
            &json!({
                "title": "By owner", "viewType": "board",
                "fields": {
                    "viewType": "board", "groupBy": "owner",
                    "filter": {"operation": "and", "filters": [
                        {"propertyId": "owner", "condition": "includes", "values": [person]},
                    ]},
                    "visibleGroups": [person, ""],
                    "hiddenGroups": [person],
                    "collapsedGroups": [person],
                    "groupCalculations": {(person.clone()): {"calculation": "count", "propertyId": "owner"}},
                },
            }),
        )
        .await
        .check_created();
    let token = ts.share(&board.id).await;
    let path = format!("/shared/{}/blocks?all=true&readToken={token}", board.id);
    let before = ts.anonymous().get(&path).await;
    before.check_ok();
    assert!(
        !before.text().contains(&person),
        "the person property initially hides its filter and groups"
    );
    ts.client
        .patch(
            &format!("{TEST_WS}/containers/{}", board.id),
            &json!({"deletedCardProperties": ["owner"]}),
        )
        .await
        .check_ok();
    let after = ts.anonymous().get(&path).await;
    ts.teardown().await;
    after.check_ok();
    assert!(
        !after.text().contains(&person),
        "deleting the property must not publish its former users: {}",
        after.text()
    );
}

/// An upload named in the body belongs to its workspace just as a path target does.
#[tokio::test]
async fn archive_import_hides_an_upload_of_a_sibling_workspace() {
    let ts = Setup::native().start().await.init_basic().await;
    let opened: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/archive/uploads"),
            &json!({"name": "plan.fvoci", "size": 0}),
        )
        .await
        .created_json();
    let path = format!("{ROOT_WS}/archive/import");
    let sibling = ts
        .client
        .post(&path, &json!({"uploadId": opened["uploadId"]}))
        .await;
    let absent = ts
        .client
        .post(&path, &json!({"uploadId": "no-such-upload"}))
        .await;
    ts.teardown().await;
    absent.check_not_found();
    assert_eq!(
        (sibling.status, sibling.error_message()),
        (absent.status, absent.error_message()),
        "a sibling upload is absent under this workspace: {}",
        sibling.text()
    );
}

/// Admission must precede decoding, including on the archive route in the inventory.
#[tokio::test]
async fn archive_import_checks_workspace_admission_before_decoding() {
    let ts = Setup::native().start().await.init_basic().await;
    let mut answers = Vec::new();
    for workspace in [HIDDEN_WORKSPACE_ID, UNKNOWN_ID] {
        let answer = ts
            .client
            .send_raw_body(Method::POST, &format!("/w/{workspace}/archive/import"), "{")
            .await;
        answers.push((workspace, answer.status, answer.error_message()));
    }
    ts.teardown().await;
    assert!(
        answers
            .iter()
            .all(|(_, status, _)| *status == StatusCode::NOT_FOUND),
        "an unjoined or absent workspace is 404 before the body is decoded: {answers:?}"
    );
}

/// Counts the request's store reads without clock noise or extra metrics scrapes.
#[derive(Debug)]
struct ReadCalls(Arc<AtomicU64>);

impl Observer for ReadCalls {
    fn called(&self, call: Call) {
        if call.access == Access::Read {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn waited_for_connection(&self, _db: DbType, _waited: Duration) {}
}

/// Different database work can disclose existence despite identical HTTP 404s.
/// Count I/O deterministically; this test makes no claim about measured wall time.
#[tokio::test]
async fn hidden_resource_identifiers_do_not_add_database_reads() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(common::TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let card: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "A private card"}),
        )
        .await
        .created_json();
    let card_id = card["id"].as_str().expect("a card identifier");
    let reads = Arc::new(AtomicU64::new(0));
    assert!(
        ts.app
            .store()
            .observe(Box::new(ReadCalls(Arc::clone(&reads)))),
        "the test installs the store's only observer"
    );
    let mut differences = Vec::new();
    for (hidden, absent) in [
        (
            format!("{ROOT_WS}/boards/{}/views", board.id),
            format!("{ROOT_WS}/boards/{UNKNOWN_ID}/views"),
        ),
        (
            format!("{ROOT_WS}/cards/{card_id}"),
            format!("{ROOT_WS}/cards/{UNKNOWN_ID}"),
        ),
    ] {
        // Warm both paths before observing them; the second user cannot read the board.
        ts.client2.get(&hidden).await.check_not_found();
        ts.client2.get(&absent).await.check_not_found();
        let before = reads.load(Ordering::SeqCst);
        let concealed = ts.client2.get(&hidden).await;
        let hidden_reads = reads.load(Ordering::SeqCst) - before;
        let before = reads.load(Ordering::SeqCst);
        let missing = ts.client2.get(&absent).await;
        let absent_reads = reads.load(Ordering::SeqCst) - before;
        concealed.check_not_found();
        missing.check_not_found();
        assert_eq!(
            concealed.error_message(),
            missing.error_message(),
            "the same refusal is returned for {hidden} and {absent}"
        );
        if hidden_reads != absent_reads {
            differences.push(format!(
                "{hidden}: hidden={hidden_reads}, absent={absent_reads}"
            ));
        }
    }
    ts.teardown().await;
    assert!(
        differences.is_empty(),
        "concealed and absent identifiers must not select different read paths: {differences:?}"
    );
}
