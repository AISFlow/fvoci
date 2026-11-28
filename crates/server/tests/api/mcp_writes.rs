//! What an agent writes through the MCP server: as the routes write it, kept as the
//! agent's version after the body before, added to a body people type in without
//! taking what they typed, and refused where the routes refuse it.

use serde_json::{Value, json};

use crate::api_tokens::agent_writing;
use crate::body_versions::versions;
use crate::card_recurrence::{DUE, STATUS, chores, weekly};
use crate::common::{
    Client, ROOT_WORKSPACE_ID, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS, UNKNOWN_ID,
};
use crate::mcp::{agent_of, call, called, refused, request, send};
use crate::rooms::{QUIET, paragraphs, two_in_a_room};

/// Whether `text` says each of `words`.
#[track_caller]
fn check_says(text: &Value, words: &[&str]) {
    let text = text
        .as_str()
        .map_or_else(|| text.to_string(), str::to_owned);
    for word in words {
        assert!(text.contains(word), "{word} in {text}");
    }
}

/// Whether one of `kept` is the agent's, named by its token.
#[track_caller]
fn check_an_agents_version(kept: &[Value]) {
    assert!(
        kept.iter()
            .any(|version| version["cause"] == "agent" && version["agentTokenName"] == "agent"),
        "{kept:?}"
    );
}

#[tokio::test]
async fn markdown_added_to_a_card_people_type_in_keeps_what_they_typed_and_is_the_agents() {
    let th = Setup::native().start().await.init_basic().await;
    let (table, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["before"])).await;
    let epochs = (one.epoch, two.epoch);
    let unsent = one.editor().append_paragraph("typed by one");
    let agent = agent_writing(&th, &th.client).await;
    let card = |more: Value| {
        let mut arguments = json!({"workspaceId": TEST_WORKSPACE_ID, "cardId": table.card_id});
        arguments
            .as_object_mut()
            .expect("an object")
            .extend(more.as_object().expect("an object").clone());
        arguments
    };

    // Written whole, the body would lose what they typed.
    let read: Value = th
        .client
        .get(&format!("{TEST_WS}/cards/{}", table.card_id))
        .await
        .ok_json();
    let problem = refused(
        &agent,
        "save_card",
        &card(json!({"bodyMarkdown": "whole", "readAt": read["updatedAt"]})),
    )
    .await;
    assert_eq!(problem["reason"], "room_has_active_editors", "{problem}");

    let added = called(
        &agent,
        "save_card",
        &card(json!({"appendMarkdown": "From the **agent**"})),
    )
    .await;
    check_says(&added["bodyMarkdown"], &["before", "agent"]);
    while one.step(QUIET).await {}
    while two.step(QUIET).await {}
    assert_eq!(
        (one.epoch, two.epoch),
        epochs,
        "nobody loads the document anew"
    );
    assert!(one.editor.is_some() && two.editor.is_some(), "no docReset");
    one.send_update(&unsent).await;
    one.text("docAck").await;
    while two.step(QUIET).await {}
    assert_eq!(one.body(), two.body(), "the editors hold one document");
    th.app
        .collab()
        .close(&th.app, &table.card_id)
        .await
        .expect("closed");
    let stored = called(
        &agent,
        "get_item",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "card", "id": table.card_id}),
    )
    .await;
    check_says(
        &stored["item"]["bodyMarkdown"],
        &["before", "typed by one", "agent"],
    );
    check_an_agents_version(&versions(&th.client, &table.card_id).await);

    // The fields first, then what is added; and never both bodies at once.
    let renamed = called(
        &agent,
        "save_card",
        &card(json!({"title": "renamed", "appendMarkdown": "and more"})),
    )
    .await;
    assert_eq!(renamed["title"], "renamed", "{renamed}");
    check_says(&renamed["bodyMarkdown"], &["agent", "and more"]);
    let params = json!({"name": "save_card", "arguments":
        card(json!({"bodyMarkdown": "a", "appendMarkdown": "b", "readAt": 1}))});
    let answer: Value = agent
        .post(
            "/mcp",
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params}),
        )
        .await
        .ok_json();
    assert_eq!(answer["error"]["code"], -32_602, "{answer}");

    th.teardown().await;
}

/// Makes a wiki or course of `kind` in the test workspace as `client`.
async fn container(client: &Client, kind: &str, visibility: &str) -> String {
    let made: Value = client
        .post(
            &format!("{TEST_WS}/{kind}"),
            &json!({"visibility": visibility, "title": kind}),
        )
        .await
        .created_json();
    made["id"].as_str().expect("an id").to_owned()
}

#[tokio::test]
async fn a_page_is_made_written_and_added_to_as_the_agents_and_commented_on() {
    let th = Setup::native().start().await.init_basic().await;
    let wiki = container(&th.client, "wikis", "private").await;
    let agent = agent_writing(&th, &th.client).await;
    let page = |more: Value| {
        let mut arguments =
            json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "page", "containerId": wiki});
        arguments
            .as_object_mut()
            .expect("an object")
            .extend(more.as_object().expect("an object").clone());
        arguments
    };

    let made = called(
        &agent,
        "save_page_or_unit",
        &page(json!({"title": "Notes", "bodyMarkdown": "# Notes\n\nfirst"})),
    )
    .await;
    assert_eq!(made["title"], "Notes", "{made}");
    check_says(&made["bodyMarkdown"], &["Notes", "first"]);
    let page_id = made["id"].as_str().expect("an id").to_owned();
    let added = called(
        &agent,
        "save_page_or_unit",
        &page(json!({"id": page_id, "appendMarkdown": "second"})),
    )
    .await;
    check_says(&added["bodyMarkdown"], &["first", "second"]);
    let kept: Vec<Value> = th
        .client
        .get(&format!(
            "{TEST_WS}/wikis/{wiki}/pages/{page_id}/body-versions"
        ))
        .await
        .ok_json();
    check_an_agents_version(&kept);
    // Written whole as it was read: refused once it changed since.
    let problem = refused(
        &agent,
        "save_page_or_unit",
        &page(json!({"id": page_id, "bodyMarkdown": "over", "readAt": made["updatedAt"]})),
    )
    .await;
    assert_eq!(problem["status"], 409, "{problem}");
    // What is added to it takes none: it keeps what changed since. Nothing is added.
    let alone = json!({"name": "save_page_or_unit",
                       "arguments": page(json!({"id": page_id, "appendMarkdown": "late",
                                                "readAt": made["updatedAt"]}))});
    let answer: Value = send(&agent, &request("tools/call", &alone)).await.ok_json();
    assert_eq!(answer["error"]["code"], -32602, "{answer}");
    let read = called(
        &agent,
        "get_item",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "page", "containerId": wiki,
                "id": page_id}),
    )
    .await;
    assert!(
        !read["item"]["bodyMarkdown"].to_string().contains("late"),
        "{read}"
    );

    let comment = json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "page",
                         "containerId": wiki, "itemId": page_id});
    let mut posted = comment.clone();
    posted["markdown"] = json!("Read it, **twice**.");
    let written = called(&agent, "post_comment", &posted).await;
    assert_eq!(written["agentTokenName"], "agent", "{written}");
    check_says(&written["markdown"], &["twice"]);
    let listed = called(&agent, "list_comments", &comment).await;
    assert_eq!(listed["comments"][0]["id"], written["id"], "{listed}");
    let by_route: Vec<Value> = th
        .client
        .get(&format!("{TEST_WS}/wikis/{wiki}/pages/{page_id}/comments"))
        .await
        .ok_json();
    assert_eq!(by_route[0]["fields"]["agentTokenName"], "agent");
    let mut empty = comment.clone();
    empty["markdown"] = json!("   ");
    let problem = refused(&agent, "post_comment", &empty).await;
    assert_eq!(problem["status"], 400, "{problem}");

    th.teardown().await;
}

#[tokio::test]
async fn progress_is_recorded_in_a_course_the_caller_joined_and_refused_in_one_they_did_not() {
    let th = Setup::native().start().await.init_basic().await;
    let mine = container(&th.client, "courses", "private").await;
    let theirs = container(&th.client2, "courses", "open").await;
    let their_unit: Value = th
        .client2
        .post(
            &format!("{TEST_WS}/courses/{theirs}/units"),
            &json!({"kind": "unit", "title": "Theirs"}),
        )
        .await
        .created_json();
    let their_unit = their_unit["id"].as_str().expect("an id");
    let agent = agent_writing(&th, &th.client).await;
    let reader = agent_of(&th, &th.client, "read", &[TEST_WORKSPACE_ID]).await;

    let made = called(
        &agent,
        "save_page_or_unit",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "unit", "containerId": mine,
                "title": "Week 1", "bodyMarkdown": "Read chapter one.",
                "progress": {"completed": true}}),
    )
    .await;
    check_says(&made["bodyMarkdown"], &["chapter one"]);
    assert!(made["myProgress"]["completedAt"].is_i64(), "{made}");
    let outline = called(
        &reader,
        "get_outline",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "containerId": mine}),
    )
    .await;
    assert_eq!(outline["kind"], "course", "{outline}");
    assert!(outline["units"][0]["updatedAt"].is_i64(), "{outline}");
    assert_eq!(outline["myProgress"].as_array().map(Vec::len), Some(1));

    // Open to them, and not joined: read, with no progress of theirs, and none recorded.
    let open = called(
        &reader,
        "get_outline",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "containerId": theirs}),
    )
    .await;
    assert_eq!(open["myProgress"], json!([]), "{open}");
    let problem = refused(
        &agent,
        "save_page_or_unit",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "unit", "containerId": theirs,
                "id": their_unit, "progress": {"completed": true}}),
    )
    .await;
    assert_eq!(problem["status"], 403, "{problem}");
    let result = call(
        &reader,
        "get_item",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "unit", "containerId": theirs,
                "id": their_unit}),
    )
    .await;
    assert_eq!(result["isError"], false, "{result}");

    th.teardown().await;
}

#[tokio::test]
async fn what_a_tool_refuses_it_writes_nothing_of() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, fvoci_model::ContainerVisibility::Private)
        .await;
    let course = container(&th.client, "courses", "private").await;
    let agent = agent_writing(&th, &th.client).await;

    // A page whose body names a file its wiki does not have is not made.
    let wiki = container(&th.client, "wikis", "private").await;
    let problem = refused(
        &agent,
        "save_page_or_unit",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "page", "containerId": wiki,
                "title": "half", "bodyMarkdown": format!("[gone](fvoci-file://{UNKNOWN_ID})")}),
    )
    .await;
    assert_eq!(problem["status"], 400, "{problem}");
    let outline = called(
        &agent,
        "get_outline",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "containerId": wiki}),
    )
    .await;
    assert_eq!(outline["pages"], json!([]), "no page was made: {outline}");
    // A card made with what is added is made with it as its body; one that is there is
    // not renamed when what it would add is refused.
    let made = called(
        &agent,
        "save_card",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "boardId": board.id, "title": "kept",
                "appendMarkdown": "made with it"}),
    )
    .await;
    check_says(&made["bodyMarkdown"], &["made with it"]);
    let problem = refused(
        &agent,
        "save_card",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "cardId": made["id"], "title": "renamed",
                "appendMarkdown": "   "}),
    )
    .await;
    assert_eq!(problem["status"], 400, "{problem}");
    let card: Value = th
        .client
        .get(&format!(
            "{TEST_WS}/cards/{}",
            made["id"].as_str().expect("an id")
        ))
        .await
        .ok_json();
    assert_eq!(card["title"], "kept", "{card}");

    // A chapter has no body: one is not made with one.
    for body in [json!({"bodyMarkdown": "x"}), json!({"appendMarkdown": "x"})] {
        let mut arguments = json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "unit",
                                   "containerId": course, "unitKind": "chapter",
                                   "title": "Part one"});
        arguments
            .as_object_mut()
            .expect("an object")
            .extend(body.as_object().expect("an object").clone());
        let problem = refused(&agent, "save_page_or_unit", &arguments).await;
        assert_eq!(problem["status"], 400, "{body}: {problem}");
    }
    // Nor is one with progress: a chapter has none.
    let problem = refused(
        &agent,
        "save_page_or_unit",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "unit", "containerId": course,
                "unitKind": "chapter", "title": "Part two", "progress": {"completed": true}}),
    )
    .await;
    assert_eq!(problem["status"], 400, "{problem}");
    let outline = called(
        &agent,
        "get_outline",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "containerId": course}),
    )
    .await;
    assert_eq!(
        outline["units"],
        json!([]),
        "no chapter was made: {outline}"
    );

    th.teardown().await;
}

#[tokio::test]
async fn a_card_made_with_its_body_keeps_the_body_as_the_agents() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, fvoci_model::ContainerVisibility::Private)
        .await;
    let agent = agent_writing(&th, &th.client).await;
    for body in [
        json!({"bodyMarkdown": "Made **whole**."}),
        json!({"appendMarkdown": "Made by adding."}),
    ] {
        let mut arguments =
            json!({"workspaceId": TEST_WORKSPACE_ID, "boardId": board.id, "title": "new"});
        arguments
            .as_object_mut()
            .expect("an object")
            .extend(body.as_object().expect("an object").clone());
        let made = called(&agent, "save_card", &arguments).await;
        check_says(&made["bodyMarkdown"], &["Made"]);
        let card_id = made["id"].as_str().expect("an id");
        check_an_agents_version(&versions(&th.client, card_id).await);
    }

    th.teardown().await;
}

#[tokio::test]
async fn a_comment_that_names_nobody_or_someone_unseen_is_kept_and_read_without_their_name() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, fvoci_model::ContainerVisibility::Private)
        .await;
    let card: Value = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "talked of"}),
        )
        .await
        .created_json();
    let agent = agent_writing(&th, &th.client).await;
    let mut named = vec![(UNKNOWN_ID.to_owned(), "ghost".to_owned())];
    // An account of another organization is somebody, whom the reader does not see.
    if fvoci_store::testing::db_type_of_the_tests() == fvoci_store::DbType::Postgres {
        let (theirs, _, _) = crate::workspaces::another_organization(&th).await;
        let mut stranger = fvoci_model::User {
            id: fvoci_model::new_id(),
            username: "stranger.elsewhere".to_owned(),
            email: "stranger.elsewhere@sample.com".to_owned(),
            ..fvoci_model::User::default()
        };
        theirs
            .create_user(&mut stranger)
            .await
            .expect("their user is stored");
        named.push((stranger.id, stranger.username));
    }
    let on_the_card = json!({"workspaceId": TEST_WORKSPACE_ID, "kind": "card",
                             "containerId": board.id, "itemId": card["id"]});
    for (id, name) in &named {
        let mut posted = on_the_card.clone();
        posted["markdown"] = json!(format!("ask @[{name}](fvoci-user://{id})"));
        let comment = called(&agent, "post_comment", &posted).await;
        let said = comment["markdown"].as_str().expect("Markdown");
        assert!(said.contains(&format!("fvoci-user://{id}")), "{comment}");
        assert!(!said.contains(name.as_str()), "{comment}");
    }
    let listed = called(&agent, "list_comments", &on_the_card).await;
    let comments = listed["comments"].as_array().expect("comments");
    assert_eq!(comments.len(), named.len(), "{listed}");
    for (_, name) in &named {
        assert!(!listed.to_string().contains(name.as_str()), "{listed}");
    }

    th.teardown().await;
}

#[tokio::test]
async fn an_agent_makes_a_card_repeat_completes_cards_alike_and_reads_what_was_done() {
    let th = Setup::native().start().await.init_basic().await;
    let board_id = chores(&th.client).await;
    let agent = agent_writing(&th, &th.client).await;
    let made = called(
        &agent,
        "save_card",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "boardId": board_id, "title": "water",
                "recurrence": weekly(), "properties": {DUE: {"from": "2099-01-15"}}}),
    )
    .await;
    assert_eq!(made["recurrence"], weekly(), "{made}");
    let id = made["id"].as_str().expect("an identifier").to_owned();
    let other = called(
        &agent,
        "save_card",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "boardId": board_id, "title": "sweep"}),
    )
    .await;
    let stopped = called(
        &agent,
        "save_card",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "cardId": other["id"], "recurrence": null}),
    )
    .await;
    assert_eq!(stopped.get("recurrence"), None, "{stopped}");

    let cards = |ids: Value| {
        json!({"workspaceId": TEST_WORKSPACE_ID, "boardId": board_id, "cardIds": ids,
               "properties": {STATUS: "done"}})
    };
    let done = called(&agent, "save_cards", &cards(json!([id, other["id"]]))).await;
    assert_eq!(done["cards"].as_array().map(Vec::len), Some(2), "{done}");
    let listed: Value = th
        .client
        .get(&format!("{TEST_WS}/boards/{board_id}/cards"))
        .await
        .ok_json();
    assert_eq!(listed.as_array().map(Vec::len), Some(3), "one next card");
    let problem = refused(&agent, "save_cards", &cards(json!([]))).await;
    assert_eq!(problem["status"], 400, "{problem}");

    let activity = called(
        &agent,
        "list_activity",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "cardId": id, "perPage": 1}),
    )
    .await;
    let lines = activity["lines"].as_array().expect("lines");
    assert_eq!(lines.len(), 1, "{activity}");
    assert_eq!(
        (
            &lines[0]["kind"],
            &lines[0]["after"],
            &lines[0]["actorTokenName"]
        ),
        (&json!("property"), &json!("done"), &json!("agent")),
        "{activity}"
    );
    let second = called(
        &agent,
        "list_activity",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "cardId": id, "page": 2, "perPage": 1}),
    )
    .await;
    assert_eq!(second["lines"][0]["kind"], "created", "{second}");
    th.teardown().await;
}

/// More cards than a batch takes are refused, and nothing of them is written; the
/// activity of a card the agent's person may not read, or of another workspace, is
/// not found, as that of no card is.
#[tokio::test]
async fn too_many_cards_and_a_card_out_of_reach_are_refused_to_an_agent() {
    let th = Setup::native().start().await.init_basic().await;
    let board_id = chores(&th.client).await;
    let agent = agent_writing(&th, &th.client).await;
    let ids: Vec<String> = (0..201).map(|_| fvoci_model::new_id()).collect();
    let problem = refused(
        &agent,
        "save_cards",
        &json!({"workspaceId": TEST_WORKSPACE_ID, "boardId": board_id, "cardIds": ids,
                "properties": {STATUS: "done"}}),
    )
    .await;
    assert_eq!(problem["status"], 400, "{problem}");

    let theirs = chores(&th.client2).await;
    let card = |client: &Client, path: String| {
        let client = client.clone();
        async move {
            let made: Value = client
                .post(&path, &json!({"title": "theirs"}))
                .await
                .created_json();
            made["id"].as_str().expect("an identifier").to_owned()
        }
    };
    let private = card(&th.client2, format!("{TEST_WS}/boards/{theirs}/cards")).await;
    let elsewhere: Value = th
        .client
        .post(
            &format!("{ROOT_WS}/boards"),
            &json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "open", "title": "root"}),
        )
        .await
        .created_json();
    let elsewhere_id = elsewhere["id"].as_str().expect("an identifier");
    let root_card = card(&th.client, format!("{ROOT_WS}/boards/{elsewhere_id}/cards")).await;
    let activity = |card_id: &str| json!({"workspaceId": TEST_WORKSPACE_ID, "cardId": card_id});
    let unknown = refused(&agent, "list_activity", &activity(UNKNOWN_ID)).await;
    assert_eq!(unknown["status"], 404, "{unknown}");
    for card_id in [&private, &root_card] {
        let problem = refused(&agent, "list_activity", &activity(card_id)).await;
        assert_eq!(problem["status"], 404, "{card_id}: {problem}");
        assert_eq!(problem["detail"], unknown["detail"], "{problem} {unknown}");
    }
    th.teardown().await;
}
