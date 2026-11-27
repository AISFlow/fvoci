//! How many connections may read one shared board by its public link, in all and from
//! one address: one more is told `blocksUnsubscribed` and is not counted among the
//! board's anonymous readers; the members' own connections are not held to it.

use serde_json::{Value, json};

use crate::common::{Setup, TEST_WORKSPACE_ID, TestServer};
use crate::rooms::{paragraphs, table};
use crate::websocket::{PATIENCE, QUIET, Socket};

/// A server that believes the addresses its local proxy names, with these limits.
async fn server(limits: fn(&mut fvoci_server::config::Configuration)) -> TestServer {
    // Local mode: public links are on.
    Setup::local_mode()
        .configure(|config| {
            config.trusted_proxies = vec!["127.0.0.1".to_owned(), "::1".to_owned()];
        })
        .configure(limits)
        .start()
        .await
        .init_basic()
        .await
}

/// A board with a card, shared: the board, the card and the link's read token.
async fn shared(th: &TestServer) -> (String, String, String) {
    let t = table(th, &paragraphs(&["hello"])).await;
    let token = th.share(&t.board_id).await;
    (t.board_id, t.card_id, token)
}

/// A connection from `address` that reads the board by its link: follows the card and
/// visits the board. What it is told of its subscription within a while, if anything.
async fn reader(
    th: &TestServer,
    address: &str,
    (board_id, card_id, token): &(String, String, String),
) -> (Socket, Option<Value>) {
    let mut socket = Socket::connect_from(th, address).await;
    socket
        .send(json!({
            "type": "subscribeBlocks", "workspaceId": TEST_WORKSPACE_ID,
            "blockIds": [card_id], "readToken": token,
        }))
        .await;
    socket
        .send(json!({
            "type": "setPresence", "workspaceId": TEST_WORKSPACE_ID,
            "containerId": board_id, "readToken": token,
        }))
        .await;
    let told = socket.unsubscribed_message(QUIET).await;
    (socket, told)
}

/// The board's member, on the board: what they are told of its anonymous readers.
async fn member_on(th: &TestServer, board_id: &str) -> Socket {
    let mut member = Socket::connect_as(&th.client).await;
    member
        .send(json!({
            "type": "setPresence", "workspaceId": TEST_WORKSPACE_ID, "containerId": board_id,
        }))
        .await;
    member
}

/// The anonymous count of the latest roster the member is sent, once it is `count`.
async fn anonymous_reaches(member: &mut Socket, count: u64) {
    let mut last = Value::Null;
    for _ in 0..20 {
        let Some(message) = member.next_json(PATIENCE).await else {
            break;
        };
        if message["type"] == "presenceChanged" {
            if message["anonymousCount"] == count {
                return;
            }
            last = message;
        }
    }
    panic!("the anonymous count never reached {count}; last roster {last}");
}

/// The anonymous count of the first roster a new member connection on the board is
/// sent: the count as it is now, not one that is yet to come.
async fn anonymous_now(th: &TestServer, board_id: &str) -> Value {
    let mut member = member_on(th, board_id).await;
    loop {
        let message = member
            .next_json(PATIENCE)
            .await
            .expect("a roster comes to who comes onto the board");
        if message["type"] == "presenceChanged" {
            return message["anonymousCount"].clone();
        }
    }
}

/// A connection from `address` that only visits the board by its link, as the web
/// app's page of a public link does.
async fn visitor(th: &TestServer, address: &str, board_id: &str, token: &str) -> Socket {
    let mut socket = Socket::connect_from(th, address).await;
    visit(&mut socket, board_id, token).await;
    socket
}

/// Visits the board by its link again: what a page does every 30 seconds.
async fn visit(socket: &mut Socket, board_id: &str, token: &str) {
    socket
        .send(json!({
            "type": "setPresence", "workspaceId": TEST_WORKSPACE_ID,
            "containerId": board_id, "readToken": token,
        }))
        .await;
}

/// Follows the card by the link; what the connection is told within a while, if
/// anything.
async fn follow(socket: &mut Socket, card_id: &str, token: &str) -> Option<Value> {
    socket
        .send(json!({
            "type": "subscribeBlocks", "workspaceId": TEST_WORKSPACE_ID,
            "blockIds": [card_id], "readToken": token,
        }))
        .await;
    socket.unsubscribed_message(QUIET).await
}

fn is_refusal(told: Option<&Value>) -> bool {
    told.is_some_and(|told| {
        told["type"] == "blocksUnsubscribed" && told["reason"] == "public link connections full"
    })
}

#[tokio::test]
async fn one_address_reads_a_board_up_to_its_limit_and_others_go_on() {
    let th = server(|config| {
        config.public_link_connections_per_container = 5;
        config.public_link_connections_per_address = 2;
    })
    .await;
    let board = shared(&th).await;
    let mut member = member_on(&th, &board.0).await;
    let mut readers = Vec::new();
    for _ in 0..2 {
        let (socket, told) = reader(&th, "203.0.113.7", &board).await;
        assert_eq!(told, None, "a reader within the limits is taken");
        readers.push(socket);
    }
    anonymous_reaches(&mut member, 2).await;
    let (_third, told) = reader(&th, "203.0.113.7", &board).await;
    assert!(
        is_refusal(told.as_ref()),
        "the third from one address: {told:?}"
    );
    assert_eq!(
        anonymous_now(&th, &board.0).await,
        2,
        "the refused reader is not counted"
    );
    let (_elsewhere, told) = reader(&th, "198.51.100.1", &board).await;
    assert_eq!(told, None, "another address is taken");
    anonymous_reaches(&mut member, 3).await;
    th.teardown().await;
}

#[tokio::test]
async fn a_board_reads_up_to_its_limit_and_takes_one_again_when_one_leaves() {
    let th = server(|config| {
        config.public_link_connections_per_container = 2;
        config.public_link_connections_per_address = 10;
    })
    .await;
    let board = shared(&th).await;
    let mut member = member_on(&th, &board.0).await;
    let (first, told) = reader(&th, "192.0.2.1", &board).await;
    assert_eq!(told, None);
    let (_second, told) = reader(&th, "192.0.2.2", &board).await;
    assert_eq!(told, None);
    anonymous_reaches(&mut member, 2).await;
    let (_third, told) = reader(&th, "192.0.2.3", &board).await;
    assert!(is_refusal(told.as_ref()), "the board's third: {told:?}");
    // The member's own connection is no reader by the link: still on the board.
    assert_eq!(anonymous_now(&th, &board.0).await, 2);
    drop(first);
    anonymous_reaches(&mut member, 1).await;
    let (_fourth, told) = reader(&th, "192.0.2.4", &board).await;
    assert_eq!(told, None, "a place freed is taken again");
    anonymous_reaches(&mut member, 2).await;
    th.teardown().await;
}

/// Readers that only visit, as the web app's page of a public link does: the board
/// takes as many as it may, the next is not counted, and is taken on its renewal once a
/// reader left.
#[tokio::test]
async fn visitors_alone_fill_a_board_and_a_renewal_takes_a_place_freed() {
    let th = server(|config| {
        config.public_link_connections_per_container = 2;
        config.public_link_connections_per_address = 10;
    })
    .await;
    let (board_id, _, token) = shared(&th).await;
    let mut member = member_on(&th, &board_id).await;
    let mut first = visitor(&th, "192.0.2.1", &board_id, &token).await;
    let _second = visitor(&th, "192.0.2.2", &board_id, &token).await;
    anonymous_reaches(&mut member, 2).await;
    let mut third = visitor(&th, "192.0.2.3", &board_id, &token).await;
    assert_eq!(
        anonymous_now(&th, &board_id).await,
        2,
        "the third visitor is not counted"
    );
    // The first leaves the board, its connection open.
    first
        .send(json!({"type": "setPresence", "containerId": ""}))
        .await;
    anonymous_reaches(&mut member, 1).await;
    visit(&mut third, &board_id, &token).await;
    anonymous_reaches(&mut member, 2).await;
    assert_eq!(anonymous_now(&th, &board_id).await, 2);
    th.teardown().await;
}

/// A follower that stops following the board frees its place for the next.
#[tokio::test]
async fn unsubscribing_frees_a_place() {
    let th = server(|config| {
        config.public_link_connections_per_container = 1;
        config.public_link_connections_per_address = 10;
    })
    .await;
    let (_, card_id, token) = shared(&th).await;
    let mut first = Socket::connect_from(&th, "192.0.2.1").await;
    assert_eq!(follow(&mut first, &card_id, &token).await, None);
    let mut second = Socket::connect_from(&th, "192.0.2.2").await;
    let told = follow(&mut second, &card_id, &token).await;
    assert!(is_refusal(told.as_ref()), "the board's second: {told:?}");
    first
        .send(json!({
            "type": "unsubscribeBlocks", "workspaceId": TEST_WORKSPACE_ID,
            "blockIds": [card_id], "readToken": token,
        }))
        .await;
    // Unsubscribing has no answer: the second asks until it is taken.
    let mut admitted = false;
    for _ in 0..20 {
        if follow(&mut second, &card_id, &token).await.is_none() {
            admitted = true;
            break;
        }
    }
    assert!(admitted, "the place the first freed was not taken");
    th.teardown().await;
}
