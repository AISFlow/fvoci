//! Integration tests for system endpoints (`ping`, `clientConfig`) and HTTP
//! routing middleware: request-forgery checks, cache headers, SPA fallback, and base path.

use fvoci_model::ContainerVisibility;
use reqwest::header::CONTENT_TYPE;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::ROOT_WS;
use crate::common::TEST_WS;
use crate::common::{
    Client, PASSWORD, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, USER1_USERNAME,
};

/// Retired `/api/v1/hello` returns 404.
#[tokio::test]
async fn retired_hello_route_returns_not_found() {
    let th = Setup::native().start().await;

    let response = Client::send(th.client.raw_request(Method::GET, "/api/v1/hello")).await;

    response.check_error(StatusCode::NOT_FOUND, "route not found");
    th.teardown().await;
}

/// `GET /ping` returns camelCase fvoci server metadata fields.
#[tokio::test]
async fn ping_returns_metadata_on_success() {
    let th = Setup::native().start().await;

    let response = Client::send(th.client.raw_request(Method::GET, "/api/v1/ping")).await;

    let metadata = response.ok_json::<serde_json::Value>();
    assert_eq!(
        metadata["version"],
        json!(env!("CARGO_PKG_VERSION")),
        "the server's own version: {metadata}"
    );
    let db_type = metadata["dbType"].as_str().unwrap_or_default();
    assert!(db_type == "sqlite" || db_type == "postgres", "{metadata}");
    assert!(
        !metadata["dbVersion"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "the database reports a version: {metadata}"
    );
    let mut keys: Vec<&str> = metadata
        .as_object()
        .into_iter()
        .flat_map(|object| object.keys().map(String::as_str))
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["buildDate", "commit", "dbType", "dbVersion", "version"],
        "expected ping metadata keys"
    );
    th.teardown().await;
}

/// `GET /clientConfig` returns the public client configuration without telemetry fields.
#[tokio::test]
async fn get_client_config() {
    let th = Setup::local_mode()
        .configure(|config| {
            config.name_display = "username".to_owned();
            config
                .feature_flags
                .insert("newBoardsEditor".to_owned(), "true".to_owned());
            config.max_file_bytes = 1234;
        })
        .start()
        .await;

    // Without a session: the login page needs the configuration.
    let response = th.anonymous().get("/client-config").await;

    assert_eq!(
        response.ok_json::<serde_json::Value>(),
        json!({
            "allowPublicLinks": true,
            "nameDisplay": "username",
            "featureFlags": {"newBoardsEditor": "true"},
            "maxFileBytes": 1234,
            "maxImportBytes": 268_435_456,
            "searchEnabled": true,
            "trashRetentionDays": 30,
        }),
        "the client configuration"
    );
    th.teardown().await;
}

/// Reading needs no header of the request-forgery check: a link or an image tag can
/// point at the API. What changes something does; see the `cookies` module.
#[tokio::test]
async fn reading_needs_no_requested_with_header() {
    let th = Setup::native().start().await.init_basic().await;

    for path in [
        "/api/v1/client-config",
        "/api/v1/users/me",
        "/api/v1/users/me/workspaces",
    ] {
        let request = th
            .client
            .raw_request(Method::GET, path)
            .bearer_auth(&th.client.token);
        Client::send(request).await.check_ok();
    }
    Client::send(th.client.raw_request(Method::GET, "/api/v1/ping"))
        .await
        .check_ok();
    th.teardown().await;
}

/// API answers must not be kept by a cache; that is a deliberate difference.
#[tokio::test]
async fn api_responses_are_not_cacheable() {
    let th = Setup::native().start().await.init_basic().await;

    let ok = th.client.get("/users/me").await;
    assert_eq!(
        ok.header("cache-control"),
        "no-store",
        "a successful answer"
    );
    assert_eq!(ok.header("content-type"), "application/json", "is JSON");

    let refused = th.anonymous().get("/users/me").await;
    refused.check_unauthorized();
    assert_eq!(
        refused.header("cache-control"),
        "no-store",
        "an error answer"
    );
    assert_eq!(
        refused.header("content-type"),
        "application/problem+json",
        "is a problem"
    );
    th.teardown().await;
}

/// The `type` of a problem is the address of a page of text about its kind, under the
/// path the server is served at, and every kind the contract lists has its page.
#[tokio::test]
async fn the_type_of_a_problem_is_the_address_of_a_page_about_its_kind() {
    let contract: Value = serde_json::from_str(include_str!("../../../../contracts/openapi.json"))
        .expect("the contract is JSON");
    let listed = contract["components"]["schemas"]["Problem"]["x-problem-types"]
        .as_array()
        .expect("the kinds of problem of the contract");
    assert!(listed.len() > 20, "{} kinds", listed.len());

    for base_path in ["", &format!("{ROOT_WS}/boards")] {
        let th = Setup::native().base_path(base_path).start().await;
        let origin = th
            .base_url
            .strip_suffix(base_path)
            .expect("the address ends with the base path");
        let http = reqwest::Client::new();
        let page_at = |address: String| Client::send(http.get(format!("{origin}{address}")));

        // An error answer points at the page of its kind.
        let refused = th.anonymous().get("/users/me").await;
        refused.check_unauthorized();
        let address = refused.problem_type();
        assert_eq!(address, format!("{base_path}/problems/unauthorized"));
        let page = page_at(address.clone()).await;
        page.check_ok();
        assert_eq!(page.header("content-type"), "text/plain; charset=utf-8");
        assert!(
            page.text().starts_with(&format!(
                "No valid session\n\ntype: {address}\nstatus: 401\n\n"
            )),
            "{}",
            page.text()
        );

        // Every kind the contract lists has its page, and the list names each.
        let list = page_at(format!("{base_path}/problems")).await;
        list.check_ok();
        let list = list.text();
        for kind in listed {
            let title = kind["title"].as_str().expect("a title");
            let address = format!("{base_path}{}", kind["type"].as_str().expect("a type"));
            let page = page_at(address.clone()).await;
            page.check_ok();
            let page = page.text();
            assert!(
                page.starts_with(&format!("{title}\n\ntype: {address}\n")),
                "{page}"
            );
            if let Some(reason) = kind["reason"].as_str() {
                assert!(page.contains(&format!("\nreason: {reason}\n")), "{page}");
            }
            assert!(
                list.contains(&format!("\n{address}  {title}\n")),
                "the list names {address}: {list}"
            );
        }
        assert_eq!(
            list.lines()
                .filter(|line| line.contains("/problems/"))
                .count(),
            listed.len(),
            "the list has no kind the contract does not: {list}"
        );

        // A name of no kind is not found, as a problem.
        let missing = page_at(format!("{base_path}/problems/no-such-kind")).await;
        missing.check_error(StatusCode::NOT_FOUND, "problem type not found");
        assert_eq!(
            missing.problem_type(),
            format!("{base_path}/problems/not-found")
        );
        th.teardown().await;
    }
}

/// What the framework refuses before a handler runs is answered in the API's format
/// too: a parameter of the path that is no text.
#[tokio::test]
async fn a_path_that_is_no_text_is_a_problem_like_any_other_error() {
    let th = Setup::native().start().await.init_basic().await;
    for path in [
        &format!("{ROOT_WS}/containers/%FF"),
        &format!("{ROOT_WS}/containers/%FF/public-link"),
        &format!("{ROOT_WS}/users/%C3%28"),
    ] {
        let response = th.client.get(path).await;
        response.check_error(StatusCode::BAD_REQUEST, "the request is malformed");
        assert_eq!(response.header("cache-control"), "no-store", "{path}");
    }
    th.teardown().await;
}

/// An operation without a result answers 204: no body, and no `Content-Type` for the
/// body that is not there, whichever route it is and whatever else its answer carries
/// (a login and a logout set a cookie).
#[tokio::test]
async fn an_answer_without_a_result_has_no_body_and_no_content_type() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await
        .id;
    let card: Value = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{board}/cards"),
            &json!({"title": "card"}),
        )
        .await
        .created_json();
    let card = card["id"].as_str().expect("the card has an identifier");

    // In an order in which each finds what it needs: the logout ends the session the
    // others are sent with.
    let requests = [
        (
            Method::POST,
            format!("{TEST_WS}/boards/{board}/cards/batch-delete"),
            json!({"cardIds": [card]}),
        ),
        (
            Method::DELETE,
            format!("{TEST_WS}/containers/{board}"),
            Value::Null,
        ),
        (
            Method::POST,
            "/login".to_owned(),
            json!({"username": USER1_USERNAME, "password": PASSWORD}),
        ),
        (Method::POST, "/logout".to_owned(), Value::Null),
    ];
    for (method, path, body) in requests {
        let request = th.client.request(method.clone(), &path);
        let request = if body.is_null() {
            request
        } else {
            request.json(&body)
        };
        let response = Client::send(request).await;

        assert_eq!(
            response.status,
            StatusCode::NO_CONTENT,
            "{method} {path}: {}",
            response.text()
        );
        assert!(
            response.body.is_empty(),
            "{method} {path}: no body, but {:?}",
            response.text()
        );
        assert!(
            !response.headers.contains_key(CONTENT_TYPE),
            "{method} {path}: no content type, but {:?}",
            response.header("content-type")
        );
    }
    th.teardown().await;
}

/// With a base path, everything lives under it and the page's base address names it.
#[tokio::test]
async fn base_path_prefixes_every_route() {
    let th = Setup::native()
        .base_path(&format!("{ROOT_WS}/boards"))
        .configure(|config| config.server_url = "https://example.com/boards".to_owned())
        .start()
        .await
        .init_basic()
        .await;

    // The clients already talk to `/boards/api/v1`.
    th.client.get("/users/me").await.check_ok();
    let index = Client::send(th.client.raw_request(Method::GET, "/board/x")).await;
    index.check_ok();
    assert!(
        index.text().contains("<base href=\"/boards/\">"),
        "{}",
        index.text()
    );

    // Outside the prefix there is nothing.
    let root = th.base_url.trim_end_matches(&format!("{ROOT_WS}/boards"));
    let outside = reqwest::get(format!("{root}/api/v1/client-config"))
        .await
        .expect("the server answers");
    assert_eq!(
        outside.status(),
        StatusCode::NOT_FOUND,
        "outside the base path"
    );
    th.teardown().await;
}

/// A base path that is no absolute path is refused at startup with an error. It would
/// otherwise end in a panic of the router.
#[tokio::test]
async fn startup_refuses_a_malformed_base_path() {
    for base_path in [
        "boards",
        "/containers/{workspace}",
        &format!("{ROOT_WS}/boards/*rest"),
        // The router would read these as patterns, or never match them.
        "/:boards",
        &format!("{ROOT_WS}/containers/:workspace"),
        &format!("{ROOT_WS}/boards?x=1"),
        "/boards#top",
        "/a//b",
        "/my boards",
        "/boards\\x",
    ] {
        let error = Setup::native()
            .base_path(base_path)
            .try_start()
            .await
            .err()
            .unwrap_or_else(|| panic!("the base path {base_path:?} must be refused"));
        assert!(
            error.to_string().contains("FVOCI_BASE_PATH"),
            "{base_path:?}: {error}"
        );
    }

    // A trailing slash is fine, as before: the base path is the same without it.
    let th = Setup::native()
        .base_path(&format!("{ROOT_WS}/boards/"))
        .start()
        .await;
    let ping = format!("{}/api/v1/ping", th.base_url.trim_end_matches('/'));
    assert!(
        ping.ends_with(&format!("{ROOT_WS}/boards/api/v1/ping")),
        "{ping}"
    );
    let response = Client::send(reqwest::Client::new().get(ping)).await;
    response.check_ok();
    assert_eq!(
        response.value()["version"],
        json!(env!("CARGO_PKG_VERSION")),
        "the API, not the web app's page"
    );
    th.teardown().await;
}

/// A handler finishes what it started when its client goes away. The framework would
/// otherwise drop it at its next pause: a board would exist without being filed under
/// its creator's default category. Handlers are spawned so they run to completion.
#[tokio::test]
async fn a_request_is_finished_when_its_client_disconnects() {
    use tokio::io::AsyncWriteExt as _;

    let th = Setup::native().start().await.init_basic().await;
    let address = th.base_url.trim_start_matches("http://").to_owned();

    for round in 0..20 {
        let title = format!("abandoned {round}");
        let body =
            json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": title})
                .to_string();
        let request = format!(
            "POST /api/v1/w/{ROOT_WORKSPACE_ID}/boards HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {}\r\n\
             X-Requested-With: XMLHttpRequest\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{body}",
            th.client.token,
            body.len()
        );
        let mut stream = tokio::net::TcpStream::connect(&address)
            .await
            .expect("the server accepts the connection");
        stream
            .write_all(request.as_bytes())
            .await
            .expect("the request is sent");
        // Gone before the answer.
        drop(stream);
    }

    // A request whose connection closed before its handler began does nothing at all.
    // Every board that was created must also have been filed under the default
    // category. A board is stored before it is filed, so under load the two counts can
    // stay apart for a while: wait until nothing changes any more and every created
    // board is filed, for ten seconds at most. A board that is never filed fails.
    let mut settled = (0, 0, 0);
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let boards = th.client.get(&format!("{ROOT_WS}/boards")).await.value();
        let created: Vec<&str> = boards
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|board| board["id"].as_str())
            .collect();
        let categories = th
            .client
            .get(&format!("{ROOT_WS}/categories"))
            .await
            .value();
        let filed: Vec<&str> = categories
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|category| {
                category["containerMetadata"]
                    .as_array()
                    .into_iter()
                    .flatten()
            })
            .filter_map(|metadata| metadata["containerId"].as_str())
            .collect();
        let counts = (
            created.len(),
            created.iter().filter(|id| filed.contains(id)).count(),
            categories.as_array().map_or(0, Vec::len),
        );
        if counts == settled && counts.0 > 0 && counts.0 == counts.1 {
            break;
        }
        settled = counts;
    }
    let (created, filed, categories) = settled;
    assert!(created > 0, "some of the abandoned requests were started");
    assert_eq!(filed, created, "every created board was filed");
    assert_eq!(categories, 1, "under the one default category");
    th.teardown().await;
}

/// Static file names are percent-decoded once: a name with a space or a percent sign
/// is found.
#[tokio::test]
async fn static_file_names_are_decoded_once() {
    let th = Setup::native().start().await;
    let directory = std::path::Path::new(&th.app.config().web_app_path).join("static");
    std::fs::write(directory.join("with space.js"), "spaced").expect("a file is written");
    std::fs::write(directory.join("100%.txt"), "percent").expect("a file is written");

    let spaced = Client::send(
        th.client
            .raw_request(Method::GET, "/static/with%20space.js"),
    )
    .await;
    spaced.check_ok();
    assert_eq!(spaced.text(), "spaced", "the name with a space");
    let percent = Client::send(th.client.raw_request(Method::GET, "/static/100%25.txt")).await;
    percent.check_ok();
    assert_eq!(percent.text(), "percent", "the name with a percent sign");

    // Leaving the static directory stays impossible.
    let path = "/static/..%2Findex.html";
    let response = Client::send(th.client.raw_request(Method::GET, path)).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}");
    th.teardown().await;
}
