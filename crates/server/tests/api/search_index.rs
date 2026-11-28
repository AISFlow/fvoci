//! Keeping the search server up with what is stored: every change a search would find
//! differently is noted with its write and sent by the upkeep, also after the search
//! server was away; an index the changes were not kept for is made again and
//! everything sent.

use std::collections::BTreeMap;
use std::time::Duration;

use fvoci_model::{Block, BlockType, Container, ContainerVisibility, SearchHit, new_id};
use fvoci_server::search::tend;
use fvoci_store::DbType;
use fvoci_store::testing::db_type_of_the_tests;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::boards::new_board;
use crate::common::{Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::content_search::{INDEX, new_card, search, started};
use crate::fake_search::{FakeSearch, digest};
use crate::metrics::{sample, scrape};
use crate::workspaces::another_organization;

async fn board(th: &TestServer, board_type: ContainerVisibility, title: &str) -> Container {
    th.client
        .create_board(&new_board(TEST_WORKSPACE_ID, board_type, title))
        .await
        .created_json()
}

/// The titles the search server holds, by the identifier of the card.
fn titles(fake: &FakeSearch) -> BTreeMap<String, String> {
    fake.documents(INDEX)
        .into_iter()
        .map(|(id, document)| {
            (
                id,
                document["title"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// How many changes the search server is still to be told.
async fn waiting(th: &TestServer) -> i64 {
    th.app
        .housekeeping()
        .store()
        .search_outbox_counts()
        .await
        .expect("counted")
        .waiting
}

/// The identifiers a search for "환불" finds.
async fn found(th: &TestServer) -> Vec<String> {
    let hits: Vec<SearchHit> = search(&th.client, TEST_WS, "환불").await.ok_json();
    hits.into_iter().map(|hit| hit.id).collect()
}

#[tokio::test]
async fn a_card_is_found_while_it_is_there_as_it_is() {
    let fake = FakeSearch::start().await;
    let th = started(&fake).await;
    let board = board(&th, ContainerVisibility::Private, "Support").await;
    let card = new_card(&th.client, &board.id, "고객 응대", "환불규정을 따른다").await;
    let block = format!("{TEST_WS}/cards/{}", card.id);

    tend(&th.app).await;
    let mut sent = fake.documents(INDEX).remove(&card.id).expect("the card");
    let org_id = th.app.store().tenant().org_id().expect("an organization");
    // The version it was written with, as the search server holds it.
    let version = sent
        .as_object_mut()
        .and_then(|document| document.remove("sent"))
        .and_then(|version| version.as_i64());
    assert_eq!(version, fake.version_of(INDEX, &card.id), "{sent}");
    assert_eq!(
        sent,
        json!({
            "orgId": org_id, "workspaceId": TEST_WORKSPACE_ID, "containerId": board.id,
            "kind": "card", "number": 1, "digest": digest("고객 응대", "환불규정을 따른다"),
            "title": "고객 응대", "text": "환불규정을 따른다",
        })
    );
    let hits: Vec<SearchHit> = search(&th.client, TEST_WS, "환불 따른다").await.ok_json();
    assert_eq!(hits.len(), 1, "found by the words it holds: {hits:?}");
    assert_eq!(hits[0].id, card.id);
    assert!(hits[0].snippet.iter().any(|part| part.matched), "{hits:?}");
    assert_eq!(waiting(&th).await, 0, "what was sent is off the list");

    // Its title changes; then a property, which no search reads.
    th.client
        .patch(&block, &json!({"title": "환불 안내"}))
        .await
        .check_ok();
    tend(&th.app).await;
    assert_eq!(titles(&fake)[&card.id], "환불 안내");
    let writes = fake.recorded_at("/_bulk").len();
    th.client
        .patch(&block, &json!({"icon": "x"}))
        .await
        .check_ok();
    tend(&th.app).await;
    assert_eq!(fake.recorded_at("/_bulk").len(), writes, "nothing to send");

    // To the trash and back.
    crate::blocks::trash_card(&th.client, &board.id, &card.id)
        .await
        .check_no_content();
    tend(&th.app).await;
    assert_eq!(titles(&fake), BTreeMap::new(), "a card in the trash");
    assert_eq!(found(&th).await, Vec::<String>::new());
    crate::blocks::restore_card(&th.client, &board.id, &card.id)
        .await
        .check_ok();
    tend(&th.app).await;
    assert_eq!(
        found(&th).await,
        std::slice::from_ref(&card.id),
        "back from the trash"
    );

    // The board, with everything on it.
    let other = new_card(&th.client, &board.id, "환불 문의", "문의").await;
    tend(&th.app).await;
    assert_eq!(titles(&fake).len(), 2);
    th.client
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await
        .check_no_content();
    tend(&th.app).await;
    assert_eq!(titles(&fake), BTreeMap::new(), "a board in the trash");
    let removal = fake
        .recorded_at("/_delete_by_query")
        .pop()
        .expect("the board's documents were removed by a query");
    assert_eq!(
        removal.body["query"]["bool"]["filter"],
        json!([{"term": {"orgId": org_id}}, {"term": {"containerId": board.id}}])
    );
    th.client
        .post_empty(&format!("{TEST_WS}/containers/{}/restore", board.id))
        .await
        .check_ok();
    tend(&th.app).await;
    let mut back = found(&th).await;
    back.sort();
    let mut both = vec![card.id.clone(), other.id.clone()];
    both.sort();
    assert_eq!(back, both, "the board is back with its cards");
    assert_eq!(waiting(&th).await, 0);
    th.teardown().await;
}

#[tokio::test]
async fn what_no_search_finds_is_not_sent() {
    let fake = FakeSearch::start().await;
    let th = started(&fake).await;
    let user1 = th.client.me().await.id;
    let board = board(&th, ContainerVisibility::Private, "Support").await;
    let card = new_card(&th.client, &board.id, "카드", "환불").await;
    let template: Container = th
        .client
        .create_board(&Container {
            is_template: true,
            ..new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "Template")
        })
        .await
        .created_json();
    new_card(&th.client, &template.id, "템플릿의 카드", "환불").await;
    let mut comment = Block {
        id: new_id(),
        container_id: board.id.clone(),
        parent_id: card.id.clone(),
        block_type: BlockType::Comment,
        title: "환불 댓글".to_owned(),
        ..Block::default()
    };
    th.app
        .store()
        .insert_block(&mut comment, &user1)
        .await
        .expect("the comment is stored");

    tend(&th.app).await;
    let sent = titles(&fake);
    let left = waiting(&th).await;
    th.teardown().await;

    assert_eq!(
        sent.into_keys().collect::<Vec<_>>(),
        [card.id],
        "the card, and neither the template's card nor the comment"
    );
    assert_eq!(left, 0);
}

#[tokio::test]
async fn a_server_without_a_search_server_notes_nothing() {
    let th = Setup::native().start().await.init_basic().await;
    let board = board(&th, ContainerVisibility::Private, "Support").await;
    let card = new_card(&th.client, &board.id, "카드", "환불").await;
    crate::blocks::trash_card(&th.client, &board.id, &card.id)
        .await
        .check_no_content();
    th.client
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await
        .check_no_content();
    tend(&th.app).await;

    let left = waiting(&th).await;
    th.teardown().await;

    assert_eq!(left, 0);
}

#[tokio::test]
async fn what_was_written_while_the_search_server_was_away_is_sent_when_it_is_back() {
    let fake = FakeSearch::start().await;
    let th = Setup::native()
        .search(fake.settings())
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    tend(&th.app).await;
    let board = board(&th, ContainerVisibility::Private, "Support").await;

    fake.go_down();
    let card = new_card(&th.client, &board.id, "카드", "환불").await;
    tend(&th.app).await;
    let unavailable = search(&th.client, TEST_WS, "환불").await;
    let ready = th.anonymous().get("/ready").await;
    let kept = waiting(&th).await;
    let down = scrape(&th).await;
    fake.come_up();
    // The upkeep leaves a search server that took no request alone for a second.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    tend(&th.app).await;
    let back = found(&th).await;
    let left = waiting(&th).await;
    let up = scrape(&th).await;
    th.teardown().await;

    unavailable.check_status(StatusCode::SERVICE_UNAVAILABLE);
    ready.check_ok();
    assert_eq!(kept, 1, "the change waits");
    assert_eq!(back, [card.id]);
    assert_eq!(left, 0);
    let changes = |body: &str, outcome: &str| {
        sample(body, "fvoci_search_changes_total", &[("outcome", outcome)])
    };
    assert!(changes(&down, "unavailable") >= Some(1.0), "{down}");
    assert_eq!(changes(&down, "done"), Some(0.0));
    assert_eq!(changes(&up, "done"), Some(1.0));
    let asked = |outcome: &str| sample(&up, "fvoci_search_requests_total", &[("outcome", outcome)]);
    assert_eq!(
        (asked("unavailable"), asked("answered")),
        (Some(1.0), Some(1.0))
    );
    assert_eq!(
        sample(&up, "fvoci_search_outbox", &[("state", "waiting")]),
        Some(0.0)
    );
}

#[tokio::test]
async fn a_document_the_search_server_refuses_is_tried_again_later() {
    let fake = FakeSearch::start().await;
    let th = started(&fake).await;
    let board = board(&th, ContainerVisibility::Private, "Support").await;
    let refused = new_card(&th.client, &board.id, "거절된 카드", "환불").await;
    let taken = new_card(&th.client, &board.id, "받은 카드", "환불").await;

    // The search server refuses one write, the first of the request.
    fake.fail_writes(1);
    tend(&th.app).await;
    let after_refusal = titles(&fake);
    let counts = th
        .app
        .housekeeping()
        .store()
        .search_outbox_counts()
        .await
        .expect("counted");
    tend(&th.app).await;
    let not_yet = titles(&fake).len();
    // Its time comes.
    th.app
        .housekeeping()
        .store()
        .execute_unchecked("UPDATE search_outbox SET due_at = 0")
        .await
        .expect("the row is due");
    tend(&th.app).await;
    let at_last = titles(&fake);
    let left = waiting(&th).await;
    th.teardown().await;

    assert_eq!(
        after_refusal.len(),
        1,
        "the other was taken: {after_refusal:?}"
    );
    assert_eq!((counts.waiting, counts.failing), (1, 1));
    assert_eq!(not_yet, 1, "not before its time");
    let mut both = vec![refused.id, taken.id];
    both.sort();
    assert_eq!(at_last.into_keys().collect::<Vec<_>>(), both);
    assert_eq!(left, 0);
}

/// The requests that made or removed the index, in their order.
fn index_requests(fake: &FakeSearch) -> Vec<String> {
    fake.recorded()
        .into_iter()
        .filter(|request| request.path == format!("/{INDEX}") && request.method != "GET")
        .map(|request| request.method)
        .collect()
}

#[tokio::test]
async fn a_server_that_gets_a_search_server_sends_everything_and_again_after_it_had_none() {
    let fake = FakeSearch::start().await;
    let mut th = Setup::native().start().await.init_basic().await;
    let board = board(&th, ContainerVisibility::Private, "Support").await;
    let first = new_card(&th.client, &board.id, "먼저 쓴 카드", "환불").await;

    // The server is given a search server: what was written before is sent.
    th.config.search = fake.settings();
    let mut th = th.killed_and_started_again(async |_| {}).await;
    tend(&th.app).await;
    let at_first = titles(&fake);
    let made_once = index_requests(&fake);
    // Started again as it is: the index is the one its changes were kept for.
    let th_again = th.killed_and_started_again(async |_| {}).await;
    tend(&th_again.app).await;
    let kept = index_requests(&fake);
    th = th_again;

    // For a while it has none, and a card goes meanwhile, unnoted.
    th.config.search = fvoci_server::config::search::SearchSettings::default();
    let mut th = th.killed_and_started_again(async |_| {}).await;
    crate::blocks::trash_card(&th.client, &board.id, &first.id)
        .await
        .check_no_content();
    let second = new_card(&th.client, &board.id, "나중에 쓴 카드", "환불").await;
    th.config.search = fake.settings();
    let th = th.killed_and_started_again(async |_| {}).await;
    tend(&th.app).await;
    let after = titles(&fake);
    let made_again = index_requests(&fake);
    th.teardown().await;

    assert_eq!(at_first.into_keys().collect::<Vec<_>>(), [first.id]);
    assert_eq!(made_once, ["PUT"]);
    assert_eq!(kept, ["PUT"], "nothing is made again without a reason");
    assert_eq!(
        after.into_keys().collect::<Vec<_>>(),
        [second.id],
        "the index holds what is stored now, and not the card that went unnoted"
    );
    assert_eq!(made_again, ["PUT", "DELETE", "PUT"]);
}

#[cfg(unix)]
#[tokio::test]
async fn the_operator_has_the_index_made_again() {
    use crate::admin::socket::{socket_path, socket_request};

    let fake = FakeSearch::start().await;
    let socket = socket_path();
    let th = Setup::native()
        .search(fake.settings())
        .admin_socket(&socket)
        .start()
        .await
        .init_basic()
        .await;
    tend(&th.app).await;
    let board = board(&th, ContainerVisibility::Private, "Support").await;
    let card = new_card(&th.client, &board.id, "카드", "환불").await;
    tend(&th.app).await;
    // The index is another than the one the server made, and holds nothing.
    fake.remove_index(INDEX);
    fake.put_index(
        INDEX,
        json!({"mappings": {"_meta": {"fvoci": {"version": 8}}}}),
    );

    let reindexed = socket_request(&socket, "POST", "/api/v1/admin/search/reindex", "").await;
    let listed: Value = serde_json::from_str(&reindexed.body).expect("JSON");
    let made = index_requests(&fake);
    let definition = fake.index_definition(INDEX).expect("an index");
    let nothing_yet = titles(&fake);
    tend(&th.app).await;
    let after = titles(&fake);
    let over_http = th.client.post_empty("/admin/search/reindex").await;
    fake.go_down();
    let down = socket_request(&socket, "POST", "/api/v1/admin/search/reindex", "").await;
    th.teardown().await;

    assert_eq!(reindexed.status, 200, "{}", reindexed.body);
    assert_eq!(listed, json!({"containers": 1}));
    assert_eq!(made, ["PUT", "DELETE", "PUT"], "removed and made again");
    assert_eq!(definition["mappings"]["dynamic"], "strict", "{definition}");
    assert_eq!(
        nothing_yet,
        BTreeMap::new(),
        "sent by the upkeep, afterwards"
    );
    assert_eq!(after.into_keys().collect::<Vec<_>>(), [card.id]);
    over_http.check_not_found();
    assert_eq!(down.status, 503, "{}", down.body);
}

#[cfg(unix)]
#[tokio::test]
async fn the_operator_is_told_that_there_is_no_search_server() {
    use crate::admin::socket::{socket_path, socket_request};

    let socket = socket_path();
    let th = Setup::native()
        .admin_socket(&socket)
        .start()
        .await
        .init_basic()
        .await;

    let answer = socket_request(&socket, "POST", "/api/v1/admin/search/reindex", "").await;
    th.teardown().await;

    answer.check_error(404, "search-not-configured", "search not found");
}

/// Two organizations on one server and one index: each finds its own, by the filter
/// of its requests and by the documents' organization.
#[tokio::test]
async fn the_cards_of_two_organizations_are_sent_apart_and_found_apart() {
    // SQLite holds one organization.
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let fake = FakeSearch::start().await;
    let th = started(&fake).await;
    let ours = board(&th, ContainerVisibility::Private, "Ours").await;
    let our_card = new_card(&th.client, &ours.id, "우리 카드", "환불").await;
    let (theirs, their_workspace, their_board) = another_organization(&th).await;
    let mut their_card = Block {
        id: new_id(),
        container_id: their_board.clone(),
        block_type: BlockType::Card,
        title: "환불 그들의 카드".to_owned(),
        ..Block::default()
    };
    theirs
        .insert_block(&mut their_card, "")
        .await
        .expect("their card is stored");

    tend(&th.app).await;
    let documents = fake.documents(INDEX);
    let mine = found(&th).await;
    let asked = fake.searches().pop().expect("a search");
    th.teardown().await;

    let our_org = &documents[&our_card.id]["orgId"];
    let their_document = &documents[&their_card.id];
    assert_ne!(
        our_org, &their_document["orgId"],
        "each under its organization"
    );
    assert_eq!(their_document["workspaceId"], json!(their_workspace));
    assert_eq!(their_document["containerId"], json!(their_board));
    assert_eq!(mine, [our_card.id], "and theirs is not ours to find");
    assert_eq!(
        &asked.body["query"]["bool"]["filter"][0],
        &json!({"term": {"orgId": our_org}})
    );
}

#[tokio::test]
async fn the_server_sends_what_changed_by_itself() {
    let fake = FakeSearch::start().await;
    let mut settings = fake.settings();
    settings.upkeep_by_hand = false;
    let th = Setup::native()
        .search(settings)
        .start()
        .await
        .init_basic()
        .await;
    let board = board(&th, ContainerVisibility::Private, "Support").await;
    let card = new_card(&th.client, &board.id, "카드", "환불").await;

    // The task runs every second: the index first, then the card.
    let mut sent = BTreeMap::new();
    for _ in 0..100 {
        sent = titles(&fake);
        if !sent.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    th.teardown().await;

    assert_eq!(sent.into_keys().collect::<Vec<_>>(), [card.id]);
}

/// Both search servers make an index that is not there when a document is put into
/// it. The upkeep asks which index is there before it writes.
#[tokio::test]
async fn an_index_that_went_is_not_made_by_a_write() {
    let fake = FakeSearch::start().await;
    let th = started(&fake).await;
    let board = board(&th, ContainerVisibility::Private, "Support").await;
    let before = new_card(&th.client, &board.id, "먼저 쓴 카드", "환불").await;
    tend(&th.app).await;

    fake.remove_index(INDEX);
    let after = new_card(&th.client, &board.id, "나중에 쓴 카드", "환불").await;
    tend(&th.app).await;
    let lost = fake.index_definition(INDEX);
    tend(&th.app).await;
    let definition = fake
        .index_definition(INDEX)
        .expect("the index is made again");
    let sent = titles(&fake);
    th.teardown().await;

    assert_eq!(lost, None, "nothing was written where no index was");
    assert_eq!(definition["mappings"]["_meta"]["fvoci"]["version"], 8);
    let mut both = vec![before.id, after.id];
    both.sort();
    assert_eq!(
        sent.into_keys().collect::<Vec<_>>(),
        both,
        "everything is sent again"
    );
}

#[tokio::test]
async fn a_board_is_sent_whole_but_for_the_document_that_was_refused() {
    let fake = FakeSearch::start().await;
    let th = started(&fake).await;
    let board = board(&th, ContainerVisibility::Private, "Support").await;
    let mut cards = Vec::new();
    for number in 0..3 {
        cards.push(
            new_card(&th.client, &board.id, &format!("카드 {number}"), "환불")
                .await
                .id,
        );
    }
    cards.sort();
    tend(&th.app).await;

    // The board is listed whole, as for an index made again, and the search server
    // refuses the first document it is sent.
    fvoci_server::search::reindex(&th.app)
        .await
        .expect("the index is made again");
    fake.fail_writes(1);
    tend(&th.app).await;
    let at_first: Vec<String> = titles(&fake).into_keys().collect();
    th.app
        .housekeeping()
        .store()
        .execute_unchecked("UPDATE search_outbox SET due_at = 0")
        .await
        .expect("the row is due");
    tend(&th.app).await;
    let at_last: Vec<String> = titles(&fake).into_keys().collect();
    let left = waiting(&th).await;
    th.teardown().await;

    assert_eq!(at_first, cards[1..], "all but the one that was refused");
    assert_eq!(at_last, cards);
    assert_eq!(left, 0);
}
