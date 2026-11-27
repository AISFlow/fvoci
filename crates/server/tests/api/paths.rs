//! The path rules: what every kind of path answers, with and without a base path.
//!
//! The API has one address per route, without a trailing slash; anything else under its
//! prefix is a JSON 404, and a known path with another method a JSON 405. Outside the
//! API, a `GET` gets the web app's page, a trailing slash is redirected away, and other
//! methods are refused. None of this depends on the header the API routes check.

use std::time::Duration;

use fvoci_model::ContainerVisibility;
use reqwest::{Method, StatusCode};
use serde_json::Value;

use crate::common::{ROOT_WORKSPACE_ID, Setup, TestServer};

/// What a path answered.
#[derive(Debug)]
struct Answer {
    status: StatusCode,
    content_type: String,
    allow: String,
    location: String,
    body: String,
}

impl Answer {
    fn is_the_web_app(&self) -> bool {
        self.status == StatusCode::OK && self.body.contains("test web app")
    }

    /// The `type` of a problem body, or the empty string.
    fn problem_type(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|body| body["type"].as_str().map(str::to_owned))
            .unwrap_or_default()
    }
}

/// Sends a request without the API's header and without following redirects.
/// `address` is everything after the host, base path included.
async fn ask(origin: &str, method: Method, address: &str) -> Answer {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("an HTTP client can be built");
    let response = client
        .request(method, format!("{origin}{address}"))
        .send()
        .await
        .expect("the server answers");
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    Answer {
        status: response.status(),
        content_type: header("content-type"),
        allow: header("allow"),
        location: header("location"),
        body: response.text().await.expect("the answer has a body"),
    }
}

/// The scheme, host and port of a test server, without its base path.
fn origin_of(th: &TestServer, base: &str) -> String {
    th.base_url
        .strip_suffix(base)
        .expect("the address ends with the base path")
        .to_owned()
}

/// [`ask`] for an address put together on the spot.
async fn ask_owned(origin: &str, method: Method, address: String) -> Answer {
    ask(origin, method, &address).await
}

/// Checks every row of the path rules on the server at `origin` whose routes are under
/// `base`.
#[expect(
    clippy::too_many_lines,
    reason = "one block per row of the table of path rules"
)]
async fn check_path_rules(origin: &str, base: &str) {
    // A known API path with its method reaches the handler. No session is needed to ask
    // whether the server is there, or for the client configuration.
    let ping = ask(origin, Method::GET, &format!("{base}/api/v1/ping")).await;
    assert_eq!(ping.status, StatusCode::OK, "{base}/api/v1/ping");
    assert_eq!(ping.content_type, "application/json", "the metadata");

    // A known API path with another method: 405, with the methods the path has.
    for (method, path, allowed) in [
        (Method::DELETE, "/api/v1/client-config", "GET"),
        (Method::GET, "/api/v1/login", "POST"),
        (Method::PUT, "/api/v1/logout", "POST"),
    ] {
        let answer = ask_owned(origin, method.clone(), format!("{base}{path}")).await;
        assert_eq!(
            answer.status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {base}{path}: {}",
            answer.body
        );
        assert_eq!(
            answer.problem_type(),
            format!("{base}/problems/method-not-allowed"),
            "{method} {path}"
        );
        assert_eq!(
            answer.content_type, "application/problem+json",
            "{method} {path}"
        );
        assert!(
            answer.allow.split(',').any(|name| name.trim() == allowed),
            "{method} {path}: Allow is {:?}, which lacks {allowed}",
            answer.allow
        );
    }

    // Everything else under the API prefix: a JSON 404, never the web app's page.
    for path in [
        "/api/v1/no-such-route",
        "/api/v1/hello",
        "/api/v1/client-config/",
        "/api/v1/boards/",
        "/api/v1",
        "/api/v1/",
        "/api",
        "/api/",
        // Retired API prefix.
        "/api/v2",
        "/api/v2/boards",
        "/api/v2/clientConfig",
        "/api/v2/login",
        &format!("/api/v1/teams/{ROOT_WORKSPACE_ID}/channels"),
    ] {
        for method in [Method::GET, Method::POST] {
            let answer = ask_owned(origin, method.clone(), format!("{base}{path}")).await;
            assert_eq!(
                answer.status,
                StatusCode::NOT_FOUND,
                "{method} {base}{path}: {}",
                answer.body
            );
            assert_eq!(
                answer.problem_type(),
                format!("{base}/problems/not-found"),
                "{method} {path}"
            );
            assert_eq!(
                answer.content_type, "application/problem+json",
                "{method} {path}"
            );
        }
    }

    // The websocket endpoint is for a handshake over GET; anything else is refused in
    // the API's format.
    let answer = ask(origin, Method::POST, &format!("{base}/ws")).await;
    assert_eq!(answer.status, StatusCode::METHOD_NOT_ALLOWED, "POST /ws");
    assert_eq!(
        answer.problem_type(),
        format!("{base}/problems/method-not-allowed"),
        "POST /ws"
    );
    let answer = ask(origin, Method::GET, &format!("{base}/ws")).await;
    assert_eq!(
        answer.status,
        StatusCode::BAD_REQUEST,
        "GET /ws without a handshake: {}",
        answer.body
    );
    assert_eq!(
        answer.problem_type(),
        format!("{base}/problems/bad-request"),
        "GET /ws"
    );
    assert_eq!(answer.content_type, "application/problem+json", "GET /ws");

    // Static files: the file, or 404; no listing of the directory.
    let file = ask(origin, Method::GET, &format!("{base}/static/main.js")).await;
    assert_eq!(file.status, StatusCode::OK, "a static file");
    assert_eq!(file.body, "console.log('test');", "the file's content");
    for path in ["/static/missing.js", "/static/", "/static"] {
        let answer = ask_owned(origin, Method::GET, format!("{base}{path}")).await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "GET {base}{path}");
        assert!(!answer.is_the_web_app(), "{path} is not a screen");
    }

    // The root of the web app is the base path itself, with or without the slash.
    for path in ["", "/"] {
        if base.is_empty() && path.is_empty() {
            continue;
        }
        let answer = ask_owned(origin, Method::GET, format!("{base}{path}")).await;
        assert!(answer.is_the_web_app(), "GET {base}{path}: {answer:?}");
    }

    // Any other GET or HEAD is a screen of the web app. `/hello` and `/ping` were the
    // API's once, outside its prefix; all of the API is under the prefix now, and these
    // two are addresses like any other.
    for path in [
        "/board/abc",
        "/login",
        "/staticx/y",
        "/board/abc?x=1",
        "/hello",
        "/ping",
    ] {
        let answer = ask_owned(origin, Method::GET, format!("{base}{path}")).await;
        assert!(answer.is_the_web_app(), "GET {base}{path}: {answer:?}");
        let head = ask_owned(origin, Method::HEAD, format!("{base}{path}")).await;
        assert_eq!(head.status, StatusCode::OK, "HEAD {base}{path}");
    }

    // With a trailing slash it is redirected to the address without, query kept.
    for (path, target) in [
        ("/board/abc/", "/board/abc"),
        ("/board/abc/?x=1&y=%2F", "/board/abc?x=1&y=%2F"),
        ("/login//", "/login"),
    ] {
        for method in [Method::GET, Method::HEAD] {
            let answer = ask_owned(origin, method.clone(), format!("{base}{path}")).await;
            assert_eq!(
                answer.status,
                StatusCode::PERMANENT_REDIRECT,
                "{method} {base}{path}"
            );
            assert_eq!(
                answer.location,
                format!("{base}{target}"),
                "{method} {base}{path}"
            );
        }
    }

    // Any other method on a screen's address is refused.
    for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
        let answer = ask_owned(origin, method.clone(), format!("{base}/board/abc")).await;
        assert_eq!(
            answer.status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {base}/board/abc"
        );
        assert_eq!(answer.allow, "GET, HEAD", "{method}: what a screen takes");
    }
}

#[tokio::test]
async fn path_rules_without_a_base_path() {
    let th = Setup::native().start().await;
    check_path_rules(&origin_of(&th, ""), "").await;
    th.teardown().await;
}

#[tokio::test]
async fn path_rules_under_a_base_path() {
    let th = Setup::native().base_path("/boards").start().await;
    let origin = origin_of(&th, "/boards");
    check_path_rules(&origin, "/boards").await;

    // Outside the base path there is nothing.
    for path in [
        "/",
        "/api/v1/ping",
        "/api/v1/client-config",
        "/board/abc",
        "/boardsx",
    ] {
        let answer = ask(&origin, Method::GET, path).await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "GET {path}");
        assert!(!answer.is_the_web_app(), "{path}");
    }
    th.teardown().await;
}

/// Retired paths that have moved return a JSON 404 like any unknown API route, with or
/// without a session and regardless of whether the referenced resource exists.
#[tokio::test]
async fn retired_route_paths_are_not_found() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await.id;
    let other = th.client2.me().await.id;
    let board = th
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open)
        .await
        .id;
    let workspace = ROOT_WORKSPACE_ID;
    let file = "7unknownunknownunknownunk12.png";

    let requests = [
        (Method::GET, "/clientConfig".to_owned()),
        (
            Method::POST,
            format!("/teams/{workspace}/regenerate_signup_token"),
        ),
        (Method::POST, format!("/users/{me}/changepassword")),
        (Method::POST, format!("/users/{other}/changepassword")),
        (Method::GET, "/users/me/config".to_owned()),
        (Method::PUT, format!("/users/{me}/config")),
        (Method::PUT, format!("/users/{other}/config")),
        (Method::POST, format!("/teams/{workspace}/{board}/files")),
        (
            Method::GET,
            format!("/files/teams/{workspace}/{board}/{file}"),
        ),
        (
            Method::GET,
            format!("/files/teams/{workspace}/{board}/{file}/info"),
        ),
        (
            Method::GET,
            format!("/w/{workspace}/containers/{board}/sharing"),
        ),
        (
            Method::POST,
            format!("/w/{workspace}/containers/{board}/sharing"),
        ),
    ];
    for (method, path) in requests {
        for client in [&th.client, &th.anonymous()] {
            let response = client.send_raw_body(method.clone(), &path, "{}").await;
            assert_eq!(
                response.status,
                StatusCode::NOT_FOUND,
                "{method} {path}: {}",
                response.text()
            );
            response.check_error(StatusCode::NOT_FOUND, "route not found");
            assert_eq!(
                response.header("content-type"),
                "application/problem+json",
                "{method} {path}"
            );
        }
    }
    th.teardown().await;
}

/// A base path configured with a trailing slash is the same base path.
#[tokio::test]
async fn path_rules_under_a_base_path_configured_with_a_trailing_slash() {
    let th = Setup::native().base_path("/boards/").start().await;
    check_path_rules(&origin_of(&th, "/boards/"), "/boards").await;
    th.teardown().await;
}

/// A redirect never leaves the server: a path that would read as another host without
/// its last slash is not redirected.
#[tokio::test]
async fn a_trailing_slash_redirect_does_not_lead_to_another_host() {
    let th = Setup::native().start().await;
    let origin = origin_of(&th, "");
    for path in [
        "//evil.example/",
        "//evil.example/x/",
        "/a//b/",
        // A browser reads a backslash as a slash: `/\host` is `//host` to it.
        "/\\evil.example/",
        "/x/\\evil.example/",
    ] {
        let answer = ask(&origin, Method::GET, path).await;
        assert_ne!(
            answer.status,
            StatusCode::PERMANENT_REDIRECT,
            "GET {path} must not redirect (to {:?})",
            answer.location
        );
        assert_eq!(answer.location, "", "GET {path}");
    }
    th.teardown().await;
}
