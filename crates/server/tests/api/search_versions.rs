//! A write to the search server that lands late never undoes a later one: every
//! document is written with a version of the time it was read (`external_gte`), and
//! the search server refuses one lower than what it holds, or than what it removed.
//!
//! The same scenario runs against the fake search server, and against a real one when
//! it is named (ignored unless asked for, one server at a time):
//!
//! ```text
//! FVOCI_SEARCH_TEST_URL=http://127.0.0.1:19207 \
//!   cargo test -p fvoci-server --test api search_versions -- --ignored --nocapture
//! ```

use std::time::Duration;

use fvoci_model::{Container, ContainerVisibility, new_id};
use fvoci_server::config::search::SearchSettings;
use serde_json::{Value, json};

use crate::boards::new_board;
use crate::common::{Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::content_search::{INDEX, new_card, started};
use crate::fake_search::FakeSearch;

/// Where the test reaches the search server itself.
struct Reach {
    settings: SearchSettings,
}

impl Reach {
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let settings = &self.settings;
        let request = reqwest::Client::new()
            .request(
                method,
                format!("{}/{}{path}", settings.search_url, settings.search_index),
            )
            .timeout(Duration::from_secs(30));
        if settings.search_username.is_empty() {
            request
        } else {
            request.basic_auth(&settings.search_username, Some(&settings.search_password))
        }
    }

    /// The version and the source the index holds of `id`, if it holds it.
    async fn held(&self, id: &str) -> Option<(i64, Value)> {
        let answer: Value = self
            .request(reqwest::Method::GET, &format!("/_doc/{id}"))
            .send()
            .await
            .expect("the search server answers")
            .json()
            .await
            .expect("JSON");
        if answer["found"] != true {
            return None;
        }
        let version = answer["_version"].as_i64().expect("a version");
        Some((version, answer["_source"].clone()))
    }

    /// Writes `operation` of `id` (`index` with `source`, or `delete`) with `version`,
    /// as a `_bulk` that was sent long ago would land now. Returns its status.
    async fn write_late(&self, operation: &str, id: &str, version: i64, source: &Value) -> u64 {
        let mut body = format!(
            "{}\n",
            json!({operation: {"_id": id, "version": version, "version_type": "external_gte"}})
        );
        if operation == "index" {
            body.push_str(&source.to_string());
            body.push('\n');
        }
        let answer: Value = self
            .request(reqwest::Method::POST, "/_bulk?refresh=true")
            .header("content-type", "application/x-ndjson")
            .body(body)
            .send()
            .await
            .expect("the search server answers")
            .json()
            .await
            .expect("JSON");
        answer["items"][0][operation]["status"]
            .as_u64()
            .expect("a status")
    }
}

/// Has the upkeep send what changed until nothing is left, and the index show it.
async fn sent(th: &TestServer, reach: &Reach) {
    let everyone = th.app.housekeeping();
    for _ in 0..50 {
        fvoci_server::search::tend(&th.app).await;
        let counts = everyone.store().search_outbox_counts().await;
        if counts.expect("counted").waiting == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    reach
        .request(reqwest::Method::POST, "/_refresh")
        .send()
        .await
        .expect("the index is refreshed");
}

/// What became of each late write, and of the documents after it.
#[derive(Debug, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "what the test saw: a yes or a no for each step, compared whole"
)]
struct Seen {
    /// The version is a time in microseconds, not a count of writes.
    external: bool,
    /// The status of a put of the first title after the second was sent, and the title
    /// held then.
    after_a_change: (u64, String),
    /// The status of that put after the card was removed, and whether the card is held.
    after_the_removal: (u64, bool),
    /// Whether the other card is held once its board came back from the trash.
    back_with_its_board: bool,
    /// Whether a server that starts on an index written with versions an hour ahead of
    /// its clock writes past them.
    past_a_clock_ahead: bool,
    /// Whether a card removed with a version ahead of what the server gives (which no
    /// document of the index says) is written again past it, once the search server
    /// refused it for being lower.
    past_a_removal_ahead: bool,
}

/// The scenario, on a server `th` whose search server `reach` reaches. Returns the
/// server, started again on the way.
async fn late_writes_lose(th: TestServer, reach: &Reach) -> (TestServer, Seen) {
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "판",
        ))
        .await
        .created_json();
    let card = new_card(&th.client, &board.id, "처음 제목", "환불규정").await;
    let other = new_card(&th.client, &board.id, "남는 카드", "환불").await;
    sent(&th, reach).await;
    let (first, written) = reach.held(&card.id).await.expect("the card is sent");

    th.client
        .patch(
            &format!("{TEST_WS}/cards/{}", card.id),
            &json!({"title": "고친 제목"}),
        )
        .await
        .check_ok();
    sent(&th, reach).await;
    let late = reach.write_late("index", &card.id, first, &written).await;
    let title = reach
        .held(&card.id)
        .await
        .map(|(_, source)| source["title"].clone());
    let after_a_change = (
        late,
        title
            .and_then(|title| title.as_str().map(str::to_owned))
            .unwrap_or_default(),
    );

    crate::blocks::trash_card(&th.client, &board.id, &card.id)
        .await
        .check_no_content();
    sent(&th, reach).await;
    let late = reach.write_late("index", &card.id, first, &written).await;
    let after_the_removal = (late, reach.held(&card.id).await.is_some());

    // The board goes and comes back: its documents are removed by query, which leaves
    // each one past its version, and put again.
    th.client
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await
        .check_no_content();
    sent(&th, reach).await;
    th.client
        .post_empty(&format!("{TEST_WS}/containers/{}/restore", board.id))
        .await
        .check_ok();
    sent(&th, reach).await;
    let back_with_its_board = reach.held(&other.id).await.is_some();

    // A document written by a server whose clock was an hour ahead, and a server that
    // starts on the same database and index: it keeps the index, and writes past it.
    let ahead = first + 3_600_000_000;
    let mut planted = written.clone();
    planted["sent"] = json!(ahead);
    reach.write_late("index", &new_id(), ahead, &planted).await;
    let th = th.killed_and_started_again(async |_| {}).await;
    sent(&th, reach).await;
    th.client
        .patch(
            &format!("{TEST_WS}/cards/{}", other.id),
            &json!({"title": "다시 고친 제목"}),
        )
        .await
        .check_ok();
    sent(&th, reach).await;
    let past_a_clock_ahead = reach
        .held(&other.id)
        .await
        .is_some_and(|(version, _)| version > ahead);

    // Removed by a server whose clock was two hours ahead: the next write of it is
    // refused for its version, and sent again past what the search server says it holds.
    let removed_ahead = first + 7_200_000_000;
    reach
        .write_late("delete", &other.id, removed_ahead, &Value::Null)
        .await;
    th.client
        .patch(
            &format!("{TEST_WS}/cards/{}", other.id),
            &json!({"title": "세 번 고친 제목"}),
        )
        .await
        .check_ok();
    sent(&th, reach).await;
    let past_a_removal_ahead = reach
        .held(&other.id)
        .await
        .is_some_and(|(version, source)| {
            version > removed_ahead && source["title"] == "세 번 고친 제목"
        });

    let seen = Seen {
        external: first > 1_000_000_000_000_000,
        after_a_change,
        after_the_removal,
        back_with_its_board,
        past_a_clock_ahead,
        past_a_removal_ahead,
    };
    (th, seen)
}

/// What the scenario is to see on every search server.
fn as_it_should_be() -> Seen {
    Seen {
        external: true,
        after_a_change: (409, "고친 제목".to_owned()),
        after_the_removal: (409, false),
        back_with_its_board: true,
        past_a_clock_ahead: true,
        past_a_removal_ahead: true,
    }
}

#[tokio::test]
async fn a_late_write_loses_to_a_later_one_on_the_fake() {
    let fake = FakeSearch::start().await;
    let th = started(&fake).await;
    let reach = Reach {
        settings: SearchSettings {
            search_index: INDEX.to_owned(),
            ..fake.settings()
        },
    };

    let (th, seen) = late_writes_lose(th, &reach).await;
    th.teardown().await;

    assert_eq!(seen, as_it_should_be());
}

#[tokio::test]
#[ignore = "needs a search server with nori, named by FVOCI_SEARCH_TEST_URL"]
async fn a_late_write_loses_to_a_later_one_on_a_real_search_server() {
    let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let settings = SearchSettings {
        search_url: env("FVOCI_SEARCH_TEST_URL").expect("FVOCI_SEARCH_TEST_URL names it"),
        search_username: env("FVOCI_SEARCH_TEST_USERNAME").unwrap_or_default(),
        search_password: env("FVOCI_SEARCH_TEST_PASSWORD").unwrap_or_default(),
        search_index: format!("fvoci-test-{}", &new_id()[24..]),
        search_requests_per_second: 0,
        upkeep_by_hand: true,
        ..SearchSettings::default()
    };
    let th = Setup::native()
        .search(settings.clone())
        .start()
        .await
        .init_basic()
        .await;
    let reach = Reach { settings };
    sent(&th, &reach).await;
    let definition: Value = reach
        .request(reqwest::Method::GET, "/_settings")
        .send()
        .await
        .expect("the settings")
        .json()
        .await
        .expect("JSON");

    let (th, seen) = late_writes_lose(th, &reach).await;
    let removed = reach
        .request(reqwest::Method::DELETE, "")
        .send()
        .await
        .expect("the index of the test is removed");
    th.teardown().await;

    println!("{seen:?}");
    assert!(removed.status().is_success(), "{}", removed.status());
    let gc_deletes = definition
        .as_object()
        .and_then(|indices| indices.values().next())
        .map(|index| index["settings"]["index"]["gc_deletes"].clone());
    assert_eq!(gc_deletes, Some(json!("10m")), "{definition}");
    assert_eq!(seen, as_it_should_be());
}
