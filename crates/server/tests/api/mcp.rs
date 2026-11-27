//! The MCP server (`POST /api/v1/mcp`) as an agent speaks to it: one JSON-RPC message
//! a request, a personal API token and nothing else, the tools its scope covers and no
//! other, and the workspaces it was given and never a diary.

use fvoci_model::{ContainerVisibility, IssuedApiToken, OrgRole, WorkspaceRole};
use fvoci_store::DbType;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::api_tokens_diaries::{Space, WORD, a_space_with_a_diary};
use crate::common::{
    ApiResponse, Client, HIDDEN_WORKSPACE_ID, PASSWORD, ROOT_WORKSPACE_ID, Setup,
    TEST_WORKSPACE_ID, TEST_WS, TestServer, UNKNOWN_ID,
};
use crate::diaries::last_of;
use crate::workspaces::another_organization;

/// The path of the server.
const MCP: &str = "/mcp";

/// The tools that write.
const WRITES: [&str; 3] = ["save_card", "save_page_or_unit", "post_comment"];

/// Sends one JSON-RPC message to the server as `client`.
pub(crate) async fn send(client: &Client, message: &Value) -> ApiResponse {
    client.post(MCP, message).await
}

/// The request `method` with `params`.
pub(crate) fn request(method: &str, params: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params})
}

/// Calls the tool `name` with `arguments` as `client`, and answers with the result of
/// the call: what the tool answered, or was refused.
pub(crate) async fn call(client: &Client, name: &str, arguments: &Value) -> Value {
    let params = json!({"name": name, "arguments": arguments});
    let answer: Value = send(client, &request("tools/call", &params))
        .await
        .ok_json();
    assert_eq!(answer["id"], 7, "{answer}");
    assert!(answer.get("error").is_none(), "{name}: {answer}");
    answer["result"].clone()
}

/// What a result says: the JSON of its one text.
#[track_caller]
fn said(result: &Value) -> Value {
    let text = result["content"][0]["text"].as_str().expect("a text");
    serde_json::from_str(text).expect("the text is JSON")
}

/// Calls the tool `name`, which does what it is called for, and answers with what it
/// says.
pub(crate) async fn called(client: &Client, name: &str, arguments: &Value) -> Value {
    let result = call(client, name, arguments).await;
    assert_eq!(result["isError"], false, "{name}: {result}");
    said(&result)
}

/// Calls the tool `name`, which is refused, and answers with the problem.
pub(crate) async fn refused(client: &Client, name: &str, arguments: &Value) -> Value {
    let result = call(client, name, arguments).await;
    assert_eq!(result["isError"], true, "{name}: {result}");
    said(&result)
}

/// The code of the JSON-RPC error `client` is answered with for `message`, sent as
/// written, and the status it came with.
async fn rpc_error(client: &Client, message: &str) -> (StatusCode, i64) {
    let answer = client.send_raw_body(Method::POST, MCP, message).await;
    let code = answer.value()["error"]["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("an error for {message}: {}", answer.text()));
    (answer.status, code)
}

/// A token of `client`'s user, as an agent sends it.
pub(crate) async fn agent_of(
    ts: &TestServer,
    client: &Client,
    scope: &str,
    workspaces: &[&str],
) -> Client {
    let issued: IssuedApiToken = client
        .post(
            "/users/me/api-tokens",
            &json!({"name": "agent", "scope": scope, "workspaceIds": workspaces}),
        )
        .await
        .created_json();
    ts.client_with_token(&issued.token)
}

/// The names of the tools `client` is told of.
async fn tool_names(client: &Client) -> Vec<String> {
    let answer: Value = send(client, &request("tools/list", &json!({})))
        .await
        .ok_json();
    answer["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("a name").to_owned())
        .collect()
}

/// The identifiers of what a list of `member` has.
fn ids_of(value: &Value, member: &str) -> Vec<String> {
    value[member]
        .as_array()
        .unwrap_or_else(|| panic!("{member} in {value}"))
        .iter()
        .map(|item| item["id"].as_str().expect("an id").to_owned())
        .collect()
}

#[tokio::test]
async fn the_server_takes_a_personal_api_token_and_no_session() {
    let ts = Setup::native().start().await.init_basic().await;
    let agent = agent_of(&ts, &ts.client, "read", &[TEST_WORKSPACE_ID]).await;
    let ping = request("ping", &json!({}));
    let answer: Value = send(&agent, &ping).await.ok_json();
    assert_eq!(answer, json!({"jsonrpc": "2.0", "id": 7, "result": {}}));

    for (who, client) in [
        ("a session as a bearer", ts.client.clone()),
        ("a session in its cookie", ts.client.as_browser()),
        ("a token in the cookie", agent.as_browser()),
        ("nobody", ts.anonymous()),
        (
            "a token that stands for nobody",
            ts.client_with_token("fvoci_pat_nobodys"),
        ),
    ] {
        let answer = send(&client, &ping).await;
        assert_eq!(
            answer.status,
            StatusCode::UNAUTHORIZED,
            "{who}: {}",
            answer.text()
        );
        assert_eq!(answer.header("www-authenticate"), "Bearer", "{who}");
    }
    // It never streams, and keeps no session to end.
    for client in [&agent, &ts.anonymous(), &ts.client] {
        for method in [Method::GET, Method::DELETE] {
            let answer = Client::send(client.request(method.clone(), MCP)).await;
            assert_eq!(
                answer.status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {}",
                answer.text()
            );
        }
    }

    ts.teardown().await;
}

#[tokio::test]
async fn a_request_counts_against_the_token_and_one_that_reads_sends_little() {
    let ts = Setup::native().start().await.init_basic().await;
    let reader = agent_of(&ts, &ts.client, "read", &[TEST_WORKSPACE_ID]).await;
    let writer = agent_of(&ts, &ts.client, "write", &[TEST_WORKSPACE_ID]).await;
    // Past the 64 KiB a request without a session, or with a token that reads, sends.
    let large = request("ping", &json!({"padding": "x".repeat(70 * 1024)}));
    send(&reader, &large)
        .await
        .check_status(StatusCode::PAYLOAD_TOO_LARGE);
    send(&writer, &large).await.check_ok();

    // Refused before the token is asked of, the body counted for nothing: 120 more go.
    let ping = request("ping", &json!({}));
    for _ in 0..120 {
        send(&reader, &ping).await.check_ok();
    }
    let held = send(&reader, &ping).await;
    held.check_status(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(held.error_reason(), "token_rate_limited", "{}", held.text());
    assert!(!held.header("retry-after").is_empty(), "{:?}", held.headers);
    send(&writer, &ping).await.check_ok();

    ts.teardown().await;
}

#[tokio::test]
async fn what_is_no_request_of_the_protocol_is_answered_by_it() {
    let ts = Setup::native().start().await.init_basic().await;
    let agent = agent_of(&ts, &ts.client, "read", &[TEST_WORKSPACE_ID]).await;

    let started: Value = send(
        &agent,
        &request(
            "initialize",
            &json!({"protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": {"name": "a client", "version": "1"}}),
        ),
    )
    .await
    .ok_json();
    let started = &started["result"];
    assert_eq!(started["protocolVersion"], "2025-06-18", "{started}");
    assert_eq!(started["serverInfo"]["name"], "fvoci", "{started}");
    assert!(started["instructions"].as_str().is_some(), "{started}");
    let taken = send(
        &agent,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    taken.check_status(StatusCode::ACCEPTED);
    assert!(taken.body.is_empty(), "{}", taken.text());

    let bad_request = StatusCode::BAD_REQUEST;
    assert_eq!(rpc_error(&agent, "{").await, (bad_request, -32_700));
    let batch = r#"[{"jsonrpc": "2.0", "id": 1, "method": "ping"}]"#;
    assert_eq!(rpc_error(&agent, batch).await, (bad_request, -32_600));
    let ok = StatusCode::OK;
    let unknown = r#"{"jsonrpc": "2.0", "id": 1, "method": "resources/list"}"#;
    assert_eq!(rpc_error(&agent, unknown).await, (ok, -32_601));
    for params in [
        json!({"name": "log_time", "arguments": {}}),
        json!({"name": "get_today", "arguments": {"workspaceId": TEST_WORKSPACE_ID, "more": 1}}),
        json!({"name": "get_today", "arguments": {}}),
        json!({"name": "get_today", "arguments": []}),
    ] {
        let message = request("tools/call", &params).to_string();
        assert_eq!(rpc_error(&agent, &message).await, (ok, -32_602), "{params}");
    }

    let ping = request("ping", &json!({}));
    let spoken = Client::send(
        agent
            .request(Method::POST, MCP)
            .header("MCP-Protocol-Version", "2025-11-25")
            .json(&ping),
    )
    .await;
    spoken.check_ok();
    let unspoken = Client::send(
        agent
            .request(Method::POST, MCP)
            .header("MCP-Protocol-Version", "1999-01-01")
            .json(&ping),
    )
    .await;
    unspoken.check_status(bad_request);
    assert_eq!(
        unspoken.refused_place().as_deref(),
        Some("header MCP-Protocol-Version")
    );

    ts.teardown().await;
}

#[tokio::test]
async fn a_token_that_reads_is_told_of_no_tool_that_writes_and_is_refused_one_it_calls() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let reader = agent_of(&ts, &ts.client, "read", &[TEST_WORKSPACE_ID]).await;
    let writer = agent_of(&ts, &ts.client, "write", &[TEST_WORKSPACE_ID]).await;

    let read = tool_names(&reader).await;
    assert_eq!(read.len(), 10, "{read:?}");
    assert!(
        WRITES
            .iter()
            .all(|name| !read.iter().any(|tool| tool == name)),
        "{read:?}"
    );
    let written = tool_names(&writer).await;
    assert_eq!(written.len(), 13, "{written:?}");
    assert!(
        WRITES
            .iter()
            .all(|name| written.iter().any(|tool| tool == name))
    );

    let card = json!({"workspaceId": TEST_WORKSPACE_ID, "boardId": board.id, "title": "forced"});
    for (tool, arguments) in [
        ("save_card", card.clone()),
        // Refused for what it is before its arguments are looked at.
        ("save_card", json!({})),
        (
            "save_page_or_unit",
            json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "page",
                   "containerId": board.id, "title": "forced"}),
        ),
        (
            "post_comment",
            json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "card",
                   "containerId": board.id, "itemId": board.id, "markdown": "forced"}),
        ),
    ] {
        let problem = refused(&reader, tool, &arguments).await;
        assert_eq!(problem["status"], 403, "{tool}: {problem}");
        assert_eq!(
            problem["reason"], "token_scope_insufficient",
            "{tool}: {problem}"
        );
    }
    let cards: Value = ts
        .client
        .get(&format!("{TEST_WS}/boards/{}/cards", board.id))
        .await
        .ok_json();
    assert_eq!(cards, json!([]), "nothing was made");

    let made = called(&writer, "save_card", &card).await;
    assert_eq!(made["title"], "forced", "{made}");
    assert_eq!(made["boardId"], board.id.as_str(), "{made}");

    ts.teardown().await;
}

#[tokio::test]
async fn the_overview_names_the_workspace_the_caller_and_what_they_read_or_may_join() {
    let ts = Setup::native().start().await.init_basic().await;
    let mine = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    // An open board of the second user's, which the first reads and has not joined.
    let theirs: Value = ts
        .client2
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"title": "theirs", "visibility": "open"}),
        )
        .await
        .created_json();
    let theirs_id = theirs["id"].as_str().expect("an id");
    let agent = agent_of(&ts, &ts.client, "read", &[TEST_WORKSPACE_ID]).await;
    let me = ts.client.me().await;

    let arguments = json!({"workspaceId": TEST_WORKSPACE_ID});
    let overview = called(&agent, "get_workspace_overview", &arguments).await;
    let http: Value = agent.get(&format!("{TEST_WS}/overview")).await.ok_json();
    assert_eq!(overview, http, "the tool answers as the route");
    assert_eq!(overview["workspace"]["id"], TEST_WORKSPACE_ID, "{overview}");
    assert_eq!(overview["me"]["id"], me.id.as_str(), "{overview}");
    assert!(
        !overview.to_string().contains("email"),
        "no email: {overview}"
    );
    let containers = overview["containers"].as_array().expect("containers");
    let of = |id: &str| {
        containers
            .iter()
            .find(|container| container["id"] == id)
            .unwrap_or_else(|| panic!("{id} in {overview}"))
            .clone()
    };
    let joined = of(&mine.id);
    assert_eq!(joined["joined"], true, "{joined}");
    assert_eq!(joined["myRole"], "admin", "{joined}");
    assert!(joined["cardProperties"].is_array(), "{joined}");
    let open = of(theirs_id);
    assert_eq!(open["joined"], false, "{open}");
    assert!(open.get("myRole").is_none(), "{open}");
    assert!(open["cardProperties"].is_array(), "{open}");

    let people = called(
        &agent,
        "find_people",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "q": "user"}),
    )
    .await;
    let people = people["people"].as_array().expect("people");
    assert!(people.len() >= 2, "{people:?}");
    assert!(
        people.iter().all(|person| person.get("email").is_none()),
        "{people:?}"
    );

    ts.teardown().await;
}

/// The workspaces `list_workspaces` names for `agent`.
pub(crate) async fn listed(agent: &Client) -> Vec<String> {
    ids_of(
        &called(agent, "list_workspaces", &json!({})).await,
        "workspaces",
    )
}

#[tokio::test]
async fn a_token_lists_the_workspaces_it_was_given_and_reaches_no_diary() {
    let Space {
        th,
        own,
        diary_id,
        entry,
        file_id,
        board_id,
        agent,
        ..
    } = a_space_with_a_diary().await;
    let entry_id = last_of(&entry).to_owned();
    // Given the space alone, the token lists it alone: not the workspaces its owner
    // works in and it was not given, nor one they are not in.
    assert_eq!(listed(&agent).await, std::slice::from_ref(&own));
    let both: IssuedApiToken = th
        .client
        .post(
            "/users/me/api-tokens",
            &json!({"name": "agent", "scope": "read", "workspaceIds": [TEST_WORKSPACE_ID],
                    "allowPersonalSpace": true, "password": PASSWORD}),
        )
        .await
        .created_json();
    let both = th.client_with_token(&both.token);
    let reached = listed(&both).await;
    assert_eq!(reached, [TEST_WORKSPACE_ID.to_owned(), own.clone()]);
    assert!(
        !reached
            .iter()
            .any(|id| id == ROOT_WORKSPACE_ID || id == HIDDEN_WORKSPACE_ID)
    );

    // Of the space, the board and no diary.
    let overview = called(
        &agent,
        "get_workspace_overview",
        &json!({"workspaceId": own}),
    )
    .await;
    let containers = ids_of(&overview, "containers");
    assert!(containers.contains(&board_id), "{overview}");
    assert!(!containers.contains(&diary_id), "{overview}");
    let found = called(
        &agent,
        "search_workspace",
        &json!({"workspaceId": own, "q": WORD}),
    )
    .await;
    assert_eq!(found["hits"], json!([]), "{found}");
    let files = called(
        &agent,
        "list_files",
        &json!({"workspaceId": own, "containerId": board_id}),
    )
    .await;
    assert!(!files.to_string().contains(&file_id), "{files}");
    for (tool, arguments) in calls_on_a_diary(&own, &diary_id, &entry_id, &file_id) {
        let problem = refused(&agent, tool, &arguments).await;
        assert_eq!(problem["status"], 404, "{tool} {arguments}: {problem}");
    }

    // A workspace of another organization its owner is let into is not the token's.
    if fvoci_store::testing::db_type_of_the_tests() == DbType::Postgres {
        check_another_organization_is_not_reached(&th, &agent, &both, &own).await;
    }

    th.teardown().await;
}

/// Lets the owner of `agent` and `both` into a workspace of another organization, and
/// checks that neither token lists or enters it.
async fn check_another_organization_is_not_reached(
    th: &TestServer,
    agent: &Client,
    both: &Client,
    own: &str,
) {
    let (store, their_workspace, _) = another_organization(th).await;
    let user = th.client.me().await.id;
    store
        .add_organization_member(&user, OrgRole::Member)
        .await
        .expect("the user joins their organization");
    store
        .add_workspace_member(&their_workspace, &user, WorkspaceRole::Member)
        .await
        .expect("the user is let into their workspace");
    let theirs = format!("/w/{their_workspace}/boards");
    th.client.get(&theirs).await.check_ok();
    assert_eq!(listed(agent).await, [own], "the space alone still");
    assert!(
        !listed(both).await.contains(&their_workspace),
        "their workspace is not the token's"
    );
    let problem = refused(
        both,
        "get_workspace_overview",
        &json!({"workspaceId": their_workspace}),
    )
    .await;
    assert_eq!(problem["status"], 404, "{problem}");
}

#[tokio::test]
async fn an_outline_tells_nothing_of_what_the_caller_does_not_read() {
    let ts = Setup::native().start().await.init_basic().await;
    let theirs: Value = ts
        .client2
        .post(
            &format!("{TEST_WS}/wikis"),
            &json!({"title": "theirs", "visibility": "private"}),
        )
        .await
        .created_json();
    let agent = agent_of(&ts, &ts.client, "read", &[TEST_WORKSPACE_ID]).await;
    let outline =
        |container_id: &str| json!({"workspaceId": TEST_WORKSPACE_ID, "containerId": container_id});
    let private = refused(
        &agent,
        "get_outline",
        &outline(theirs["id"].as_str().expect("an id")),
    )
    .await;
    let unknown = refused(&agent, "get_outline", &outline(UNKNOWN_ID)).await;
    for problem in [&private, &unknown] {
        assert_eq!(problem["status"], 404, "{problem}");
    }
    assert_eq!(private["detail"], unknown["detail"], "{private} {unknown}");

    ts.teardown().await;
}

#[tokio::test]
async fn a_search_of_one_container_counts_its_hits_alone() {
    let ts = Setup::native().start().await.init_basic().await;
    let one = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let many = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let card = |title: &str| json!({"title": title});
    for _ in 0..3 {
        ts.client
            .post(
                &format!("{TEST_WS}/boards/{}/cards", many.id),
                &card("lantern lantern lantern"),
            )
            .await
            .check_created();
    }
    let alone: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", one.id),
            &card("a lantern"),
        )
        .await
        .created_json();
    assert!(
        ts.app
            .finish_background(std::time::Duration::from_secs(5))
            .await,
        "the background work ends"
    );
    let agent = agent_of(&ts, &ts.client, "read", &[TEST_WORKSPACE_ID]).await;
    let found = called(
        &agent,
        "search_workspace",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "q": "lantern", "containerId": one.id,
                "limit": 1}),
    )
    .await;
    let hits = found["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), 1, "{found}");
    assert_eq!(hits[0]["id"], alone["id"], "{found}");

    ts.teardown().await;
}

#[tokio::test]
async fn a_client_that_adds_no_header_is_told_to_send_a_token_from_any_origin_but_another() {
    let ts = Setup::native().start().await.init_basic().await;
    let agent = agent_of(&ts, &ts.client, "read", &[TEST_WORKSPACE_ID]).await;
    let ping = request("ping", &json!({}));
    // No client of the protocol sends the header a page of the server's own sends.
    for (who, client) in [
        ("nobody", ts.anonymous()),
        ("a browser's session", ts.client.as_browser()),
    ] {
        let answer = Client::send(client.bare_request(Method::POST, MCP).json(&ping)).await;
        assert_eq!(
            answer.status,
            StatusCode::UNAUTHORIZED,
            "{who}: {}",
            answer.text()
        );
        assert_eq!(answer.header("www-authenticate"), "Bearer", "{who}");
    }
    Client::send(agent.bare_request(Method::POST, MCP).json(&ping))
        .await
        .check_ok();
    // A page of another site is refused all the same.
    let foreign = agent
        .bare_request(Method::POST, MCP)
        .header("Origin", "https://elsewhere.example")
        .json(&ping);
    Client::send(foreign)
        .await
        .check_status(StatusCode::FORBIDDEN);

    ts.teardown().await;
}

#[tokio::test]
async fn the_pages_of_a_boards_cards_count_from_one_and_hold_each_card_once() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let mut made = Vec::new();
    for title in ["one", "two", "three"] {
        let card: Value = ts
            .client
            .post(
                &format!("{TEST_WS}/boards/{}/cards", board.id),
                &json!({"title": title}),
            )
            .await
            .created_json();
        made.push(card["id"].as_str().expect("an id").to_owned());
    }
    let agent = agent_of(&ts, &ts.client, "read", &[TEST_WORKSPACE_ID]).await;
    let page = |page: u32| {
        json!({"workspaceId": TEST_WORKSPACE_ID, "boardId": board.id, "page": page,
               "perPage": 2})
    };
    let first = ids_of(&called(&agent, "list_cards", &page(1)).await, "cards");
    let second = ids_of(&called(&agent, "list_cards", &page(2)).await, "cards");
    assert_eq!(first.len(), 2, "{first:?}");
    assert_eq!(second.len(), 1, "{second:?}");
    let mut all: Vec<String> = first.into_iter().chain(second).collect();
    all.sort();
    made.sort();
    assert_eq!(all, made, "each card once, none left out");
    let call = json!({"name": "list_cards", "arguments": page(0)});
    let answer: Value = send(&agent, &request("tools/call", &call)).await.ok_json();
    assert_eq!(answer["error"]["code"], -32602, "{answer}");

    ts.teardown().await;
}

/// The calls a token makes of the diary `diary_id` of its space `own`, of its entry
/// `entry_id` and of its file `file_id`: each refused as for a diary that is not there.
fn calls_on_a_diary(
    own: &str,
    diary_id: &str,
    entry_id: &str,
    file_id: &str,
) -> Vec<(&'static str, Value)> {
    vec![
        (
            "get_outline",
            json!({"workspaceId": own, "containerId": diary_id}),
        ),
        (
            "list_files",
            json!({"workspaceId": own, "containerId": diary_id}),
        ),
        (
            "list_cards",
            json!({"workspaceId": own, "boardId": diary_id}),
        ),
        (
            "get_item",
            json!({"workspaceId": own, "kind": "page", "containerId": diary_id, "id": entry_id}),
        ),
        (
            "get_item",
            json!({"workspaceId": own, "kind": "card", "id": entry_id}),
        ),
        (
            "list_comments",
            json!({"workspaceId": own, "kind": "page", "containerId": diary_id, "itemId": entry_id}),
        ),
        (
            "post_comment",
            json!({"workspaceId": own, "kind": "page", "containerId": diary_id,
               "itemId": entry_id, "markdown": "seen"}),
        ),
        (
            "save_page_or_unit",
            json!({"workspaceId": own, "kind": "page", "containerId": diary_id, "title": "in"}),
        ),
        // Whether the body names a file of the diary or one that is nowhere: nothing of
        // the diary is read before the token is refused it.
        (
            "save_page_or_unit",
            json!({"workspaceId": own, "kind": "page", "containerId": diary_id, "title": "in",
               "bodyMarkdown": format!("[x](fvoci-file://{file_id})")}),
        ),
        (
            "save_page_or_unit",
            json!({"workspaceId": own, "kind": "page", "containerId": diary_id, "title": "in",
               "bodyMarkdown": format!("[x](fvoci-file://{UNKNOWN_ID})")}),
        ),
    ]
}
