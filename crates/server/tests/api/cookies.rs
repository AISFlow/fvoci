//! How a session reaches the server: the cookie a login sets, the `Authorization`
//! header of a client that is no browser, and what keeps another site's page from
//! using a browser's cookie (the request-forgery header, the origin check, and the
//! origin check of the websocket handshake).

use fvoci_model::{Container, ContainerVisibility, User};
use reqwest::{Method, StatusCode};
use serde_json::json;

use crate::common::ROOT_WS;
use crate::common::{
    ApiResponse, Client, PASSWORD, ROOT_WORKSPACE_ID, SESSION_COOKIE, Setup, TestServer,
    USER1_USERNAME,
};

/// The origin the test servers take for their own: the default `server_url`.
const OWN_ORIGIN: &str = "http://localhost:8000";
const FOREIGN_ORIGIN: &str = "https://evil.example";

const MINUTE: i64 = 60 * 1000;

fn login_body() -> serde_json::Value {
    json!({"username": USER1_USERNAME, "password": PASSWORD})
}

fn board_body() -> serde_json::Value {
    json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": "made"})
}

#[track_caller]
fn check_csrf(response: &ApiResponse) {
    response.check_error(
        StatusCode::FORBIDDEN,
        "the X-Requested-With: XMLHttpRequest header is required",
    );
    assert_eq!(response.error_reason(), "csrf", "the reason");
}

#[track_caller]
fn check_foreign_origin(response: &ApiResponse) {
    response.check_error(
        StatusCode::FORBIDDEN,
        "requests from this origin are not accepted",
    );
    assert_eq!(response.error_reason(), "foreign_origin", "the reason");
}

/// A login answers with the session cookie and nothing in the body. The cookie cannot
/// be read by scripts, is sent with requests from the same site only, and lasts as long
/// as a session may go unused.
#[tokio::test]
async fn login_sets_the_session_cookie_and_the_body_carries_no_token() {
    let th = Setup::native()
        .configure(|config| config.session_idle_days = 3)
        .start()
        .await
        .init_basic()
        .await;

    let response = th.anonymous().post("/login", &login_body()).await;

    response.check_no_content();
    let cookies = response.set_cookies();
    assert_eq!(cookies.len(), 1, "one cookie: {cookies:?}");
    let token = response.session_cookie().expect("the session cookie");
    assert_eq!(
        cookies[0],
        format!("fvoci_session={token}; Max-Age=259200; Path=/; HttpOnly; SameSite=Lax"),
        "the cookie and its attributes, over plain HTTP"
    );
    assert_eq!(token.len(), 43, "a secret of the server");

    // The token is a session's, however it is presented.
    let me: User = th
        .client_with_token(&token)
        .get("/users/me")
        .await
        .ok_json();
    assert_eq!(me.username, USER1_USERNAME, "as a bearer token");
    let me: User = th
        .client_with_token(&token)
        .as_browser()
        .get("/users/me")
        .await
        .ok_json();
    assert_eq!(me.username, USER1_USERNAME, "in the cookie");

    // Every login is a session of its own.
    let again = th.anonymous().post("/login", &login_body()).await;
    assert_ne!(again.session_cookie(), Some(token), "another token");

    // A refused login sets nothing.
    let refused = th
        .anonymous()
        .post(
            "/login",
            &json!({"username": USER1_USERNAME, "password": "wrong password"}),
        )
        .await;
    refused.check_unauthorized();
    assert_eq!(refused.set_cookies(), Vec::<&str>::new(), "no cookie");

    th.teardown().await;
}

/// Where the server is reached over HTTPS, which `server_url` says, the cookie is
/// `Secure` and has the name browsers bind to one host, and only that name is read.
#[tokio::test]
async fn over_https_the_cookie_is_secure_and_bound_to_the_host() {
    let th = Setup::native()
        .configure(|config| config.server_url = "https://boards.example.com".to_owned())
        .start()
        .await
        .init_basic()
        .await;

    let response = th.anonymous().post("/login", &login_body()).await;
    response.check_no_content();
    let token = response.session_cookie().expect("the session cookie");
    assert_eq!(
        response.set_cookies(),
        [format!(
            "__Host-fvoci_session={token}; Max-Age=1209600; Path=/; HttpOnly; SameSite=Lax; Secure"
        )],
        "the cookie over HTTPS"
    );

    let with_cookie = |name: &str| {
        th.anonymous()
            .bare_request(Method::GET, "/users/me")
            .header("Cookie", format!("{name}={token}"))
    };
    Client::send(with_cookie("__Host-fvoci_session"))
        .await
        .check_ok();
    // The name without the prefix could be planted by another host of the domain.
    Client::send(with_cookie("fvoci_session"))
        .await
        .check_unauthorized();

    th.teardown().await;
}

/// Logging out ends the session and makes the browser forget the cookie.
#[tokio::test]
async fn logout_ends_the_session_and_clears_the_cookie() {
    let th = Setup::native().start().await.init_basic().await;
    let browser = th.client.as_browser();

    let response = browser.post_empty("/logout").await;

    response.check_no_content();
    assert_eq!(
        response.set_cookies(),
        ["fvoci_session=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax"],
        "the cookie is cleared"
    );
    browser.get("/users/me").await.check_unauthorized();
    th.client.get("/users/me").await.check_unauthorized();
    th.client2.get("/users/me").await.check_ok();

    th.teardown().await;
}

/// A request that changes something and comes with the cookie needs the header that a
/// page of another site cannot add. Reading needs none, and neither does a request with
/// the token in its `Authorization` header: no browser adds that by itself.
#[tokio::test]
async fn a_change_with_the_cookie_needs_the_request_forgery_header() {
    let th = Setup::native().start().await.init_basic().await;
    let browser = th.client.as_browser();
    let unsafe_requests = [
        (Method::POST, format!("{ROOT_WS}/boards")),
        (Method::PATCH, "/users/me/client-state".to_owned()),
        (Method::PATCH, "/users/me/settings".to_owned()),
        (
            Method::DELETE,
            format!("{ROOT_WS}/containers/{}", crate::common::UNKNOWN_ID),
        ),
        (Method::POST, "/logout".to_owned()),
    ];

    // With the cookie and without the header: refused, whatever the route.
    for (method, path) in &unsafe_requests {
        let request = browser
            .bare_request(method.clone(), path)
            .json(&board_body());
        check_csrf(&Client::send(request).await);
    }
    browser.get("/users/me").await.check_ok();

    // Reading with the cookie alone: as a link or an image tag does.
    for path in ["/users/me", "/users/me/workspaces", "/users/me/settings"] {
        Client::send(browser.bare_request(Method::GET, path))
            .await
            .check_ok();
    }
    // With the cookie and the header: the server's own pages.
    let made: Container = browser
        .post(&format!("{ROOT_WS}/boards"), &board_body())
        .await
        .created_json();
    assert_eq!(
        made.visibility,
        ContainerVisibility::Private,
        "the board was made"
    );

    // With the token in the `Authorization` header: no header of the check needed.
    let request = th
        .client
        .bare_request(Method::POST, &format!("{ROOT_WS}/boards"))
        .json(&board_body());
    Client::send(request).await.check_created();

    // Nobody's request is checked like a browser's: a forged login would sign the
    // browser in to an account of the forger's.
    let request = th
        .anonymous()
        .bare_request(Method::POST, "/login")
        .json(&login_body());
    check_csrf(&Client::send(request).await);
    let request = th
        .anonymous()
        .bare_request(Method::POST, "/register")
        .json(&json!({"username": "x", "email": "x@example.com", "password": PASSWORD}));
    check_csrf(&Client::send(request).await);
    // A header with another value is not the header.
    let request = browser
        .bare_request(Method::POST, &format!("{ROOT_WS}/boards"))
        .header("X-Requested-With", "fetch")
        .json(&board_body());
    check_csrf(&Client::send(request).await);

    th.teardown().await;
}

/// A request that changes something and names an origin must name the server's own, or
/// one the configuration allows, however it is authenticated. Reading is not judged by
/// its origin, and a request that names none is not judged by it either.
#[tokio::test]
async fn a_change_from_another_origin_is_refused() {
    let th = Setup::native()
        .configure(|config| config.allowed_origins = vec!["http://localhost:5173".to_owned()])
        .start()
        .await
        .init_basic()
        .await;
    let browser = th.client.as_browser();
    let post_from = async |client: &Client, origin: &str| {
        Client::send(
            client
                .request(Method::POST, &format!("{ROOT_WS}/boards"))
                .header("Origin", origin)
                .json(&board_body()),
        )
        .await
    };

    for origin in [
        FOREIGN_ORIGIN,
        "http://localhost:8001",
        "https://localhost:8000",
        "http://localhost.evil.example:8000",
        "null",
    ] {
        check_foreign_origin(&post_from(&browser, origin).await);
        // Also with a bearer token: a page of another origin has no business sending
        // it, whoever it got it from.
        check_foreign_origin(&post_from(&th.client, origin).await);
    }
    for origin in [OWN_ORIGIN, "HTTP://LOCALHOST:8000", "http://localhost:5173"] {
        post_from(&browser, origin).await.check_created();
    }
    // The origin check comes first: a foreign login is refused as foreign, with or
    // without the header.
    let request = th
        .anonymous()
        .request(Method::POST, "/login")
        .header("Origin", FOREIGN_ORIGIN)
        .json(&login_body());
    check_foreign_origin(&Client::send(request).await);

    // Reading: other sites may link to the API. Their requests come without the cookie
    // (it is `SameSite=Lax`), which is what keeps the answers from them.
    let request = browser
        .request(Method::GET, "/users/me")
        .header("Origin", FOREIGN_ORIGIN);
    Client::send(request).await.check_ok();

    th.teardown().await;
}

/// The token in the `Authorization` header is the one that counts when a request has
/// both: it was sent on purpose.
#[tokio::test]
async fn the_authorization_header_counts_before_the_cookie() {
    let th = Setup::native().start().await.init_basic().await;
    let token = &th.client.token;
    let other_token = &th.client2.token;
    let get_me = |bearer: &str, cookie: &str| {
        th.anonymous()
            .bare_request(Method::GET, "/users/me")
            .header("Authorization", format!("Bearer {bearer}"))
            .header("Cookie", format!("{SESSION_COOKIE}={cookie}"))
    };

    let me: User = Client::send(get_me(token, other_token)).await.ok_json();
    assert_eq!(me.username, USER1_USERNAME, "the user of the header");
    Client::send(get_me("not-a-session", token))
        .await
        .check_unauthorized();
    Client::send(get_me("", token)).await.check_unauthorized();

    // What used to be other ways to present the token are none: the `token` scheme of
    // the header and the token in the address, which ends up in logs.
    let request = th
        .anonymous()
        .bare_request(Method::GET, "/users/me")
        .header("Authorization", format!("token {token}"));
    Client::send(request).await.check_unauthorized();
    let request = th
        .anonymous()
        .bare_request(Method::GET, &format!("/users/me?access_token={token}"));
    Client::send(request).await.check_unauthorized();
    let request = th
        .anonymous()
        .bare_request(Method::GET, "/users/me")
        .header("Cookie", format!("FVOCIAUTHTOKEN={token}"));
    Client::send(request).await.check_unauthorized();

    th.teardown().await;
}

/// The cookie is sent again, with its full lifetime, when the use of the session is
/// recorded: at most every ten minutes, and only to a browser.
#[tokio::test]
async fn the_cookie_is_renewed_when_the_use_of_the_session_is_recorded() {
    let th = Setup::native().start().await.init_basic().await;
    let browser = th.client.as_browser();
    let store = th.app.store();
    let digest = fvoci_server::auth::secret::token_digest(&th.client.token);
    let session = store
        .get_session(&digest, 0, i64::MIN)
        .await
        .expect("the session is stored");
    let move_last_use_back = async |minutes: i64| {
        store
            .set_session_last_used(
                &session.id,
                fvoci_server::auth::now_millis() - minutes * MINUTE,
            )
            .await
            .expect("the last use is moved back");
    };

    let fresh = browser.get("/users/me").await;
    assert_eq!(fresh.set_cookies(), Vec::<&str>::new(), "used a moment ago");

    move_last_use_back(11).await;
    let renewed = browser.get("/users/me").await;
    assert_eq!(
        renewed.set_cookies(),
        [format!(
            "fvoci_session={}; Max-Age=1209600; Path=/; HttpOnly; SameSite=Lax",
            th.client.token
        )],
        "the same token, for another full idle period"
    );
    let after = browser.get("/users/me").await;
    assert_eq!(after.set_cookies(), Vec::<&str>::new(), "renewed just now");

    // A client that sends the token itself keeps no cookie to renew.
    move_last_use_back(11).await;
    let bearer = th.client.get("/users/me").await;
    assert_eq!(bearer.set_cookies(), Vec::<&str>::new(), "no cookie for it");

    // Where the body is large enough for the session to be asked for before the route
    // is: the renewal still reaches the answer.
    move_last_use_back(11).await;
    let large = browser
        .post(
            &format!("{ROOT_WS}/boards"),
            &json!({
                "workspaceId": ROOT_WORKSPACE_ID, "visibility": "private",
                "description": "d".repeat(70 * 1024),
            }),
        )
        .await;
    large.check_created();
    assert_eq!(large.set_cookies().len(), 1, "renewed with a large request");

    // The answer of a websocket handshake renews no cookie, so the handshake must not
    // count as the use either: the renewal would be lost, and the cookie would end
    // before the session does. It is left to the next request.
    move_last_use_back(11).await;
    let socket = browser.connect_websocket().await;
    let after_the_handshake = browser.get("/users/me").await;
    assert_eq!(
        after_the_handshake.set_cookies().len(),
        1,
        "the request after a handshake renews the cookie"
    );
    drop(socket);

    th.teardown().await;
}

/// A changed password gives the session it was changed through a new token, in the
/// cookie; the old token is no longer accepted.
#[tokio::test]
async fn a_changed_password_puts_a_new_token_in_the_cookie() {
    let th = Setup::native().start().await.init_basic().await;
    let browser = th.client.as_browser();

    let response = browser
        .post(
            "/users/me/password",
            &json!({"oldPassword": PASSWORD, "newPassword": "another password"}),
        )
        .await;

    response.check_no_content();
    let token = response.session_cookie().expect("the cookie is set anew");
    assert_ne!(token, browser.token, "a new token");
    assert_eq!(
        response.set_cookies(),
        [format!(
            "fvoci_session={token}; Max-Age=1209600; Path=/; HttpOnly; SameSite=Lax"
        )]
    );
    browser.get("/users/me").await.check_unauthorized();
    th.client_with_token(&token)
        .as_browser()
        .get("/users/me")
        .await
        .check_ok();

    th.teardown().await;
}

/// A file can be asked for by an image tag or a link: with the cookie alone. Opened as
/// a document it has no scripts and no origin, and nothing says where its reader came
/// from.
#[tokio::test]
async fn a_file_is_read_with_the_cookie_alone_and_served_sandboxed() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let uploaded = th
        .client
        .request(
            Method::POST,
            &format!("{ROOT_WS}/containers/{}/files", board.id),
        )
        .multipart(
            reqwest::multipart::Form::new().part(
                "file",
                reqwest::multipart::Part::bytes(b"<svg onload=\"alert(1)\"/>".to_vec())
                    .file_name("drawing.svg"),
            ),
        );
    let uploaded = Client::send(uploaded).await;
    uploaded.check_created();
    let file_id = uploaded.value()["fileId"]
        .as_str()
        .expect("the upload answers the file's name")
        .to_owned();
    let path = format!("{ROOT_WS}/containers/{}/files/{file_id}", board.id);

    let response = Client::send(th.client.as_browser().bare_request(Method::GET, &path)).await;

    response.check_ok();
    assert_eq!(
        response.header("content-security-policy"),
        "sandbox; frame-ancestors 'none'",
        "no scripts and no origin for the file as a document"
    );
    assert_eq!(response.header("x-content-type-options"), "nosniff");
    assert_eq!(
        response.header("referrer-policy"),
        "no-referrer",
        "nothing says where the reader came from"
    );
    // Without the cookie there is no file.
    Client::send(th.anonymous().bare_request(Method::GET, &path))
        .await
        .check_unauthorized();
    // Every answer of the server keeps the address of its pages to itself.
    assert_eq!(
        th.client.get("/users/me").await.header("referrer-policy"),
        "no-referrer"
    );

    th.teardown().await;
}

// -- the websocket handshake ----------------------------------------------------------

async fn handshake(client: &Client, headers: &[(&str, &str)]) -> Result<(), StatusCode> {
    let headers: Vec<(&str, String)> = headers
        .iter()
        .map(|(name, value)| (*name, (*value).to_owned()))
        .collect();
    client.try_connect_websocket(&headers).await.map(drop)
}

/// A websocket handshake from a browser names its origin, which must be the server's
/// own or an allowed one: a browser lets any page open the connection and sends the
/// cookie along. A handshake that names none is no browser's and needs the token in
/// its `Authorization` header.
#[tokio::test]
async fn the_websocket_handshake_is_judged_by_its_origin_and_its_session() {
    let th: TestServer = Setup::native()
        .configure(|config| config.allowed_origins = vec!["http://localhost:5173".to_owned()])
        .start()
        .await
        .init_basic()
        .await;
    let browser = th.client.as_browser();
    let nobody = th.anonymous();
    let stranger = th.client_with_token("not-a-session");

    // From a page of the server, or of an allowed origin: with the cookie, with a
    // cookie that is no session's (the page may follow a shared board), and without.
    for origin in [OWN_ORIGIN, "http://localhost:5173"] {
        for client in [&browser, &stranger.as_browser(), &nobody.as_browser()] {
            assert_eq!(
                handshake(client, &[("Origin", origin)]).await,
                Ok(()),
                "from {origin}"
            );
        }
    }
    // From a page of another origin: refused, with the user's cookie or without.
    for origin in [FOREIGN_ORIGIN, "http://localhost:8001", "null"] {
        for client in [&browser, &nobody.as_browser(), &th.client] {
            assert_eq!(
                handshake(client, &[("Origin", origin)]).await,
                Err(StatusCode::FORBIDDEN),
                "from {origin}"
            );
        }
    }
    // Without an origin: only with a token in the `Authorization` header, and then it
    // must be a session's.
    assert_eq!(handshake(&th.client, &[]).await, Ok(()), "a bearer token");
    assert_eq!(
        handshake(&stranger, &[]).await,
        Err(StatusCode::UNAUTHORIZED),
        "a bearer token that is no session's"
    );
    assert_eq!(
        handshake(&nobody, &[]).await,
        Err(StatusCode::UNAUTHORIZED),
        "nothing at all"
    );
    assert_eq!(
        handshake(
            &nobody,
            &[("Cookie", &format!("{SESSION_COOKIE}={}", th.client.token))]
        )
        .await,
        Err(StatusCode::UNAUTHORIZED),
        "a cookie without an origin is no browser's"
    );
    // A bearer token that is no session's is refused from the own origin too: it was
    // sent on purpose, and there is no falling back to "nobody".
    assert_eq!(
        handshake(&stranger, &[("Origin", OWN_ORIGIN)]).await,
        Err(StatusCode::UNAUTHORIZED),
        "an explicit token must be good"
    );

    th.teardown().await;
}
