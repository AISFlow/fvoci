//! The limits on what a request may send: the JSON body of an API route, the body of a
//! route that needs no session, and a websocket message. What exceeds a limit is refused
//! with 413 and the limit; on a websocket, which has no answers, the connection ends.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use fvoci_model::ContainerVisibility;
use reqwest::multipart::{Form, Part};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::common::TEST_WS;
use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TestServer};

/// The default of `max_request_bytes`.
const DEFAULT_JSON_LIMIT: usize = 12 * 1024 * 1024;
/// The limit of the routes that need no session.
const UNAUTHENTICATED_LIMIT: usize = 64 * 1024;
/// The limit of a websocket message.
const WEBSOCKET_LIMIT: usize = 64 * 1024;

/// A JSON object of exactly `length` bytes: `{"pad":"xxx…"}`.
fn json_of_length(length: usize) -> String {
    let frame = r#"{"pad":""}"#.len();
    assert!(length >= frame, "{length} bytes do not hold the object");
    let body = format!(r#"{{"pad":"{}"}}"#, "x".repeat(length - frame));
    assert_eq!(body.len(), length, "the body has the length asked for");
    body
}

/// Checks the one answer to a body over its limit: 413, the code, the reason and the
/// limit.
#[track_caller]
fn check_too_large(response: &ApiResponse, limit: usize) {
    response.check_error(
        StatusCode::PAYLOAD_TOO_LARGE,
        &format!("the request body is larger than {limit} bytes"),
    );
    let problem = response.value();
    assert_eq!(
        (&problem["type"], &problem["reason"], &problem["limit"]),
        (
            &json!("/problems/request-too-large"),
            &json!("request_too_large"),
            &json!(limit)
        ),
        "the answer names the reason and the limit: {problem}"
    );
}

/// Checks that a body was read and then refused for what it says, not for its size.
#[track_caller]
fn check_read_and_refused(response: &ApiResponse) {
    assert_ne!(
        response.status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        response.text()
    );
    assert!(
        response.status.is_client_error(),
        "refused for what it says: {}",
        response.text()
    );
}

/// The boards the first user sees in the test workspace.
async fn board_count(th: &TestServer) -> usize {
    let boards: Vec<Value> = th.client.get(&format!("{TEST_WS}/boards")).await.ok_json();
    boards.len()
}

/// A JSON body of exactly the limit is read; one byte more is refused with 413, on every
/// route that takes JSON and before the handler looks at anything.
#[tokio::test]
async fn a_json_body_over_the_default_limit_is_refused_with_413() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    let at_the_limit = json_of_length(DEFAULT_JSON_LIMIT);
    let response = th
        .client
        .send_raw_body(Method::POST, &format!("{TEST_WS}/boards"), &at_the_limit)
        .await;
    // It is no board.
    check_read_and_refused(&response);

    let over = json_of_length(DEFAULT_JSON_LIMIT + 1);
    let routes = [
        (Method::POST, format!("{TEST_WS}/boards")),
        (Method::PATCH, format!("{TEST_WS}/containers/{}", board.id)),
        (Method::POST, format!("{TEST_WS}/boards/{}/cards", board.id)),
        (
            Method::POST,
            format!("{TEST_WS}/boards/{}/cards/batch-patch", board.id),
        ),
        (Method::POST, format!("{TEST_WS}/boards/{}/views", board.id)),
        (Method::POST, format!("{TEST_WS}/categories")),
        (Method::POST, format!("{TEST_WS}/subscriptions")),
        (Method::POST, format!("{TEST_WS}/users")),
        // A board nobody has: the body is refused before the board is looked for.
        (Method::PATCH, format!("{TEST_WS}/containers/no-such-board")),
    ];
    for (method, path) in routes {
        let response = th.client.send_raw_body(method.clone(), &path, &over).await;
        assert_eq!(
            response.status,
            StatusCode::PAYLOAD_TOO_LARGE,
            "{method} {path}: {}",
            response.text()
        );
        check_too_large(&response, DEFAULT_JSON_LIMIT);
    }

    assert_eq!(board_count(&th).await, 1, "nothing was created");
    th.teardown().await;
}

/// The limit is the `max_request_bytes` setting.
#[tokio::test]
async fn the_json_limit_is_the_configured_one() {
    let th = Setup::native()
        .configure(|config| config.max_request_bytes = 1000)
        .start()
        .await
        .init_basic()
        .await;

    let response = th
        .client
        .send_raw_body(
            Method::POST,
            &format!("{TEST_WS}/boards"),
            &json_of_length(1001),
        )
        .await;
    check_too_large(&response, 1000);
    let response = th
        .client
        .send_raw_body(
            Method::POST,
            &format!("{TEST_WS}/boards"),
            &json_of_length(1000),
        )
        .await;
    check_read_and_refused(&response);

    // A request that fits goes through as ever.
    th.create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    assert_eq!(board_count(&th).await, 1, "the board was created");
    th.teardown().await;
}

/// A session is not needed to be refused: the body is measured before it is asked who
/// sends it, so nobody makes the server read more than the limit.
#[tokio::test]
async fn a_json_body_over_the_limit_is_refused_without_a_session_too() {
    let th = Setup::native()
        .configure(|config| config.max_request_bytes = 1000)
        .start()
        .await
        .init_basic()
        .await;

    let response = th
        .anonymous()
        .send_raw_body(
            Method::POST,
            &format!("{TEST_WS}/boards"),
            &json_of_length(1001),
        )
        .await;
    check_too_large(&response, 1000);
    th.teardown().await;
}

/// A request that claims no session at all gets no more room than the login, on any
/// route: the body is read before anyone looks at who sends it.
#[tokio::test]
async fn a_request_without_a_session_token_gets_the_small_limit_on_every_route() {
    let th = Setup::native().start().await.init_basic().await;
    let anonymous = th.anonymous();

    for (method, path) in [
        (Method::POST, &format!("{TEST_WS}/boards")),
        (
            Method::POST,
            &format!("{TEST_WS}/boards/no-such-board/cards"),
        ),
        (
            Method::PATCH,
            &format!("{TEST_WS}/containers/no-such-board"),
        ),
    ] {
        let response = anonymous
            .send_raw_body(
                method.clone(),
                path,
                &json_of_length(UNAUTHENTICATED_LIMIT + 1),
            )
            .await;
        check_too_large(&response, UNAUTHENTICATED_LIMIT);

        // Up to that, the body is read and the missing session is what is answered.
        anonymous
            .send_raw_body(method, path, &json_of_length(UNAUTHENTICATED_LIMIT))
            .await
            .check_unauthorized();
    }
    th.teardown().await;
}

/// A token that is no session's does not buy the larger allowance: a body over the small
/// limit is answered with the 401 the route would give, before any of it is read. With
/// a session the same body is read.
#[tokio::test]
async fn a_made_up_session_token_does_not_get_the_larger_limit() {
    let th = Setup::native().start().await.init_basic().await;
    let stranger = th.client_with_token("no-such-session");
    let large = json_of_length(UNAUTHENTICATED_LIMIT + 1);
    let largest = json_of_length(DEFAULT_JSON_LIMIT);

    for body in [&large, &largest] {
        let response = stranger
            .send_raw_body(Method::POST, &format!("{TEST_WS}/boards"), body)
            .await;
        response.check_error(StatusCode::UNAUTHORIZED, "invalid session token");
    }
    // Over the route's limit too: who has no session is told that first.
    let response = stranger
        .send_raw_body(
            Method::POST,
            &format!("{TEST_WS}/boards"),
            &json_of_length(DEFAULT_JSON_LIMIT + 1),
        )
        .await;
    response.check_error(StatusCode::UNAUTHORIZED, "invalid session token");

    // A small body with a made-up token is answered by the route, as ever.
    stranger
        .send_raw_body(Method::POST, &format!("{TEST_WS}/boards"), "{}")
        .await
        .check_unauthorized();

    // The session of a user gets the room.
    let response = th
        .client
        .send_raw_body(Method::POST, &format!("{TEST_WS}/boards"), &largest)
        .await;
    check_read_and_refused(&response);
    th.teardown().await;
}

/// The login and the registration, which anyone can send, read 64 KiB and no more,
/// whatever `max_request_bytes` says.
#[tokio::test]
async fn the_routes_without_a_session_have_a_smaller_limit() {
    let th = Setup::native().start().await.init_basic().await;
    let anonymous = th.anonymous();

    for path in ["/login", "/register"] {
        let response = anonymous
            .send_raw_body(
                Method::POST,
                path,
                &json_of_length(UNAUTHENTICATED_LIMIT + 1),
            )
            .await;
        check_too_large(&response, UNAUTHENTICATED_LIMIT);

        // At the limit the body is read, and refused for what it says.
        let response = anonymous
            .send_raw_body(Method::POST, path, &json_of_length(UNAUTHENTICATED_LIMIT))
            .await;
        check_read_and_refused(&response);
    }

    // The same size is no trouble for a route with a session.
    let response = th
        .client
        .send_raw_body(
            Method::POST,
            &format!("{TEST_WS}/boards"),
            &json_of_length(UNAUTHENTICATED_LIMIT + 1),
        )
        .await;
    check_read_and_refused(&response);
    th.teardown().await;
}

/// The other routes that need no session of their own doing take no body worth
/// mentioning, but are measured all the same: the logout and the password change sit
/// behind the JSON limit.
#[tokio::test]
async fn account_routes_with_a_session_are_behind_the_json_limit() {
    let th = Setup::native()
        .configure(|config| config.max_request_bytes = 1000)
        .start()
        .await
        .init_basic()
        .await;

    for path in ["/logout", "/users/me/password"] {
        let response = th
            .client
            .send_raw_body(Method::POST, path, &json_of_length(1001))
            .await;
        check_too_large(&response, 1000);
    }
    th.client.me().await;
    th.teardown().await;
}

/// The routes that take files are not behind the JSON limit: uploads have a limit of
/// their own, the `max_file_bytes` setting.
#[tokio::test]
async fn uploads_are_not_behind_the_json_limit() {
    let th = Setup::native()
        .configure(|config| config.max_request_bytes = 1000)
        .start()
        .await
        .init_basic()
        .await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    let part = Part::bytes(vec![b'x'; 8000])
        .file_name("larger.bin")
        .mime_str("application/octet-stream")
        .expect("a valid mime");
    let request = th
        .client
        .request(
            Method::POST,
            &format!("{TEST_WS}/containers/{}/files", board.id),
        )
        .multipart(Form::new().part("file", part));
    Client::send(request).await.check_created();
    th.teardown().await;
}

/// What a websocket connection yields within a moment.
#[derive(Debug, PartialEq)]
enum Outcome {
    Open,
    Closed,
}

/// Sends one text message of `length` bytes and says whether the connection is still
/// there afterwards.
async fn after_a_message_of(th: &TestServer, length: usize) -> Outcome {
    let mut stream = th.client.connect_websocket().await;
    // No command, so the server has nothing to do with it but read it.
    let text = json_of_length(length);
    if stream.send(Message::Text(text.into())).await.is_err() {
        return Outcome::Closed;
    }
    loop {
        match tokio::time::timeout(Duration::from_secs(2), stream.next()).await {
            // Nothing arrives on an idle connection that is still open.
            Err(_) => return Outcome::Open,
            Ok(None | Some(Err(_) | Ok(Message::Close(_)))) => return Outcome::Closed,
            Ok(Some(Ok(_))) => {}
        }
    }
}

/// A websocket message over 64 KiB ends the connection: there is no answer to give on a
/// websocket, and the server does not buffer what it will not read. Up to the limit a
/// message is read, and one that is no command is ignored.
#[tokio::test]
async fn a_websocket_message_over_the_limit_ends_the_connection() {
    let th = Setup::native()
        // Pings would count as traffic on the idle connection: the harness's are off.
        .start()
        .await
        .init_basic()
        .await;

    assert_eq!(
        after_a_message_of(&th, WEBSOCKET_LIMIT).await,
        Outcome::Open,
        "a message at the limit"
    );
    assert_eq!(
        after_a_message_of(&th, WEBSOCKET_LIMIT + 1).await,
        Outcome::Closed,
        "a message over the limit"
    );
    assert_eq!(
        after_a_message_of(&th, 8 * WEBSOCKET_LIMIT).await,
        Outcome::Closed,
        "a message far over the limit"
    );

    // Other connections, and the server, are not bothered.
    th.client.me().await;
    th.teardown().await;
}
