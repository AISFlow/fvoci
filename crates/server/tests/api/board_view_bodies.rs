//! The board views of a body (`boardView`): a view of a board shown in the body of a
//! card, a page or a unit, and in no other body. What it shows is told by
//! `resolveReferences` to who reads its board, and to nobody else; whoever reads a board
//! by its link is told of no view; a copy to another workspace shows the copies of the
//! views it copies and no other; and the Markdown of a body names a board view as a
//! block it does not hold as it is.

use std::time::Duration;

use fvoci_model::{Block, OrgKind, UserWorkspace, new_id};
use serde_json::{Value, json};

use crate::board_views::add_view;
use crate::collab::{editing, joined};
use crate::common::{ApiResponse, Client, ROOT_WS, Setup, TEST_WS, TestServer};
use crate::references::{HIDDEN, check_as_nothing, let_read, made, named, resolved};
use crate::references_to::card_of;
use crate::websocket::Socket;

/// A board view of the view `view_id`.
fn board_view(view_id: &str) -> Value {
    json!({"type": "boardView", "attrs": {"viewId": view_id}})
}

/// A body of a paragraph saying `text` and then the `blocks`.
fn body_of(text: &str, blocks: &[Value]) -> Value {
    let mut content =
        vec![json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})];
    content.extend(blocks.iter().cloned());
    json!({"type": "doc", "content": content})
}

/// The views the board views below `node` show, in the order they stand.
fn views_in(node: &Value) -> Vec<String> {
    let mut shown = Vec::new();
    if node["type"] == "boardView" {
        shown.push(node["attrs"]["viewId"].as_str().expect("a view").to_owned());
    }
    for child in node["content"].as_array().into_iter().flatten() {
        shown.extend(views_in(child));
    }
    shown
}

/// Writes `body` over the card or the page at `path`, as `client`: the answer.
async fn write_at(client: &Client, path: &str, body: &Value) -> ApiResponse {
    let read_at = client.get(path).await.value()["updatedAt"].clone();
    client
        .patch(path, &json!({"readAt": read_at, "body": body}))
        .await
}

/// A view of a board the user reads is told of with its board, as what it is; a view of
/// a board the user does not read (a private board they are not on, an open board to a
/// guest, a board of another workspace) or in the trash is told of as nothing.
#[tokio::test]
async fn a_board_view_is_told_to_who_reads_its_board_and_to_nobody_else() {
    let th = Setup::native().start().await.init_basic().await;
    let own = made(&th.client2, TEST_WS, "private", "own").await;
    let private = made(&th.client, TEST_WS, "private", HIDDEN).await;
    let open = made(&th.client, TEST_WS, "open", "open").await;
    let hidden_view = add_view(&th.client, &private.board, HIDDEN).await;
    let open_view = add_view(&th.client, &open.board, "Plan").await;

    let answer = resolved(&th.client, TEST_WS, &[&open_view.id, &hidden_view.id]).await;
    answer.check_ok();
    assert_eq!(
        answer.value()[0],
        json!({"id": open_view.id, "kind": "view", "title": "Plan", "icon": "",
            "containerId": open.board, "containerKind": "board",
            "containerTitle": "open (its container)", "viewType": "board"}),
    );
    assert_eq!(
        answer.value()[1]["containerTitle"],
        json!(format!("{HIDDEN} (its container)"))
    );
    // A card is told of as it was, with no board title and no kind of view.
    let card = resolved(&th.client, TEST_WS, &[&open.card]).await.value();
    let card = card[0].as_object().expect("a card");
    assert!(
        !card.contains_key("containerTitle") && !card.contains_key("viewType"),
        "{card:?}"
    );

    // A member of the workspace reads the open board, and not the private one.
    let member = &th.client2;
    check_as_nothing(member, TEST_WS, &[&hidden_view.id], &own.card, "no member").await;
    assert_eq!(
        named(&resolved(member, TEST_WS, &[&open_view.id]).await),
        [open_view.id.as_str()]
    );

    // Of another workspace, it is nothing here; in the trash, nothing at all.
    let there = made(&th.client, ROOT_WS, "open", HIDDEN).await;
    let there_view: fvoci_model::BoardViewWrite = th
        .client
        .post(
            &format!("{ROOT_WS}/boards/{}/views", there.board),
            &json!({"title": HIDDEN, "viewType": "board"}),
        )
        .await
        .created_json();
    let there_view = there_view.view;
    check_as_nothing(
        &th.client,
        TEST_WS,
        &[&there_view.id],
        &open.card,
        "elsewhere",
    )
    .await;
    let trashed = add_view(&th.client, &open.board, HIDDEN).await;
    th.client
        .delete(&format!(
            "{TEST_WS}/boards/{}/views/{}",
            open.board, trashed.id
        ))
        .await
        .check_no_content();
    check_as_nothing(
        &th.client,
        TEST_WS,
        &[&trashed.id],
        &open.card,
        "in the trash",
    )
    .await;

    // A guest reads what it was invited to: not the open board, and the private one
    // once invited.
    let guest = crate::guests::guest_of_the_workspace(&th).await;
    let views = [open_view.id.as_str(), &hidden_view.id];
    check_as_nothing(member, TEST_WS, &views, &own.card, "a guest").await;
    let_read(&th.client, &private.board, &guest).await;
    assert_eq!(
        named(&resolved(member, TEST_WS, &views).await),
        [hidden_view.id.as_str()]
    );
    th.teardown().await;
}

/// A card, a page and a unit show board views (at most ten); a comment and a diary's
/// entry show none, whether written whole or in their rooms.
#[tokio::test]
async fn a_board_view_is_shown_by_a_card_a_page_or_a_unit_and_no_other_body() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let mine = made(&th.client, TEST_WS, "private", "mine").await;
    let view = add_view(&th.client, &mine.board, "Plan").await;
    let showing = body_of("see", &[board_view(&view.id)]);
    let card_path = format!("{TEST_WS}/cards/{}", mine.card);
    write_at(&th.client, &card_path, &showing).await.check_ok();
    let page_path = format!("{TEST_WS}/wikis/{}/pages/{}", mine.wiki, mine.page);
    write_at(&th.client, &page_path, &showing).await.check_ok();
    let stored = th.client.get(&card_path).await.value();
    assert_eq!(views_in(&stored["body"]), [view.id.as_str()]);

    let ten: Vec<Value> = (0..10).map(|_| board_view(&view.id)).collect();
    write_at(&th.client, &card_path, &body_of("ten", &ten))
        .await
        .check_ok();
    let eleven: Vec<Value> = (0..11).map(|_| board_view(&view.id)).collect();
    let refused = write_at(&th.client, &card_path, &body_of("eleven", &eleven)).await;
    refused.check_bad_request();
    assert_eq!(refused.refused_at(), "/body", "{}", refused.text());

    let comment = th
        .client
        .post(
            &format!(
                "{TEST_WS}/boards/{}/cards/{}/comments",
                mine.board, mine.card
            ),
            &json!({"body": showing}),
        )
        .await;
    comment.check_bad_request();
    assert_eq!(comment.refused_at(), "/body", "{}", comment.text());

    let own = crate::own_space::own_workspace(&th.client).await.id;
    let diary = crate::diaries::a_diary(&th.client, &format!("/w/{own}")).await;
    let entry = crate::diaries::written(&th.client, &diary, &json!({"date": "2024-03-01"})).await;
    let entry_id = crate::diaries::text_of(&entry, "id");
    let entry_path = format!("{diary}/entries/{entry_id}");
    let refused = th
        .client
        .patch(
            &entry_path,
            &json!({"readAt": entry["updatedAt"], "body": showing}),
        )
        .await;
    refused.check_bad_request();
    assert_eq!(refused.refused_at(), "/body", "{}", refused.text());
    entry_room_refuses_a_board_view(&th, &own, &entry_id).await;
    th.teardown().await;
}

/// The room of a diary's entry takes a line and refuses a board view, logging nothing of
/// it; the room of a card takes it.
async fn entry_room_refuses_a_board_view(th: &TestServer, own: &str, entry_id: &str) {
    let user = th.client.me().await.id;
    let session = fvoci_model::Session {
        user_id: user.clone(),
        ..fvoci_model::Session::default()
    };
    let app = th
        .app
        .enter_workspace(&session, own)
        .await
        .expect("the membership is read")
        .expect("a member")
        .app;
    let collab = app.collab();
    let (editor, epoch) = joined(collab, &app, entry_id, 101).await;
    collab
        .take(
            &app,
            entry_id,
            epoch,
            &user,
            &editor.append_paragraph("typed"),
        )
        .await
        .expect("a line is taken");
    let refused = collab
        .take(
            &app,
            entry_id,
            epoch,
            &user,
            &editor.append_board_view(&new_id()),
        )
        .await
        .expect_err("a diary's entry shows no board view");
    assert_eq!(
        refused.reason().map(fvoci_server::error::Reason::as_str),
        Some("update_refused"),
        "{refused}"
    );
    let stored = app
        .store()
        .get_body_doc(entry_id)
        .await
        .expect("read")
        .expect("a document");
    assert_eq!(stored.log_end, 1, "the refused update is not logged");
    collab.write_body(&app, entry_id).await.expect("written");
    let entry_body = app.store().get_block(entry_id).await.expect("the entry");
    assert_eq!(
        views_in(&serde_json::to_value(&entry_body.body).expect("JSON")),
        Vec::<String>::new(),
        "the entry is written without the board view"
    );

    let e = editing(th, &Value::Null).await;
    let (editor, epoch) = joined(e.app.collab(), &e.app, &e.card_id, 102).await;
    e.app
        .collab()
        .take(
            &e.app,
            &e.card_id,
            epoch,
            &e.user1,
            &editor.append_board_view(&new_id()),
        )
        .await
        .expect("a card shows a board view");
}

/// Whoever reads a board by its link, with a session or without, is told of no view a
/// board view of its bodies shows, of the board or not, by the board's blocks or by a
/// connection that follows its card; a member on the board's own routes is told of each.
#[tokio::test]
async fn a_reader_by_a_link_is_told_of_no_view() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let shared = made(&th.client, TEST_WS, "private", "shared").await;
    let elsewhere = made(&th.client, TEST_WS, "private", HIDDEN).await;
    let own_view = add_view(&th.client, &shared.board, "Plan").await;
    let other_view = add_view(&th.client, &elsewhere.board, HIDDEN).await;
    let shown = [own_view.id.as_str(), &other_view.id];
    let body = body_of("see", &shown.map(board_view));
    let card_path = format!("{TEST_WS}/cards/{}", shared.card);
    write_at(&th.client, &card_path, &body).await.check_ok();
    let token = th.share(&shared.board).await;
    // The board's own view is among its blocks; no body names it, nor the other.
    let tells_of = |text: &str, ids: &[&str], what: &str| {
        for id in ids {
            assert!(!text.contains(id), "{what} tells of {id}: {text}");
        }
        assert!(!text.contains(HIDDEN), "{what}: {text}");
    };
    let tells_none = |text: &str, what: &str| tells_of(text, &shown, what);
    let anonymous = th.anonymous();
    for reader in [&anonymous, &th.client2, &th.client] {
        let board = &shared.board;
        let all = reader
            .get(&format!(
                "/shared/{board}/blocks?all=true&readToken={token}"
            ))
            .await;
        all.check_ok();
        tells_of(&all.text(), &[&other_view.id], "every block");
        // A body shown by a link is not one that may be stored: it names no view.
        let blocks = all.value();
        let card = blocks
            .as_array()
            .expect("a list")
            .iter()
            .find(|block| block["id"] == shared.card.as_str())
            .expect("the card");
        let card_body = &card["body"];
        assert_eq!(
            views_in(card_body),
            ["", ""],
            "each board view stays, showing nothing"
        );
        let one = reader
            .get(&format!(
                "/shared/{board}/blocks?blockId={}&readToken={token}",
                shared.card
            ))
            .await;
        one.check_ok();
        tells_none(&one.text(), "the card");
    }
    let own = th.client.get(&card_path).await.value();
    assert_eq!(views_in(&own["body"]), shown);

    // So is whoever reads a wiki's page by the wiki's link.
    let page_path = format!("{TEST_WS}/wikis/{}/pages/{}", shared.wiki, shared.page);
    write_at(&th.client, &page_path, &body).await.check_ok();
    let wiki_token = th.share(&shared.wiki).await;
    for reader in [&anonymous, &th.client2] {
        let page = reader
            .get(&format!(
                "/shared/{}/pages/{}?readToken={wiki_token}",
                shared.wiki, shared.page
            ))
            .await;
        page.check_ok();
        tells_none(&page.text(), "the page");
        assert_eq!(views_in(&page.value()["body"]), ["", ""]);
    }

    let mut socket = Socket::connect_as(&th.anonymous().as_browser()).await;
    socket
        .send(json!({
            "type": "subscribeBlocks", "workspaceId": crate::common::TEST_WORKSPACE_ID,
            "readToken": token, "blockIds": [shared.card],
        }))
        .await;
    let told = loop {
        write_at(
            &th.client,
            &card_path,
            &body_of("again", &shown.map(board_view)),
        )
        .await
        .check_ok();
        if let Some(told) = socket.next_json(Duration::from_secs(2)).await {
            break told;
        }
    };
    tells_none(&told.to_string(), "the connection");
    th.teardown().await;
}

/// A copy of a board to another workspace shows in its bodies the copies of the views it
/// copies, and in place of a board view of another board's view an empty line.
#[tokio::test]
async fn a_copy_to_another_workspace_shows_its_own_views_and_no_other() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let source = made(&th.client, TEST_WS, "open", "source").await;
    let elsewhere = made(&th.client, TEST_WS, "open", HIDDEN).await;
    let own_view = add_view(&th.client, &source.board, "Plan").await;
    let other_view = add_view(&th.client, &elsewhere.board, HIDDEN).await;
    let beside = card_of(&th.client, TEST_WS, &source.board, "beside").await;
    let mut body = body_of(
        "see",
        &[board_view(&own_view.id), board_view(&other_view.id)],
    );
    body["content"][0]["content"]
        .as_array_mut()
        .expect("a line")
        .push(json!({"type": "reference", "attrs": {"id": beside}}));
    let card_path = format!("{TEST_WS}/cards/{}", source.card);
    write_at(&th.client, &card_path, &body).await.check_ok();

    let workspaces: Vec<UserWorkspace> = th.client.get("/users/me/workspaces").await.ok_json();
    let personal = &workspaces
        .iter()
        .find(|w| w.kind == OrgKind::Personal)
        .expect("a personal workspace")
        .id;
    let copied: Value = th
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/copy-to-workspace", source.board),
            &json!({"targetWorkspaceId": personal}),
        )
        .await
        .created_json();
    let board = copied["container"]["id"].as_str().expect("the copy");
    let at = format!("/w/{personal}");
    let views: Vec<Block> = th
        .client
        .get(&format!("{at}/boards/{board}/views"))
        .await
        .ok_json();
    let plan = views
        .iter()
        .find(|view| view.title == "Plan")
        .expect("the copied view");
    let cards: Vec<Value> = th
        .client
        .get(&format!("{at}/boards/{board}/cards"))
        .await
        .ok_json();
    let id_of = |title: &str| {
        let card = cards
            .iter()
            .find(|card| card["title"] == title)
            .expect("a copied card");
        card["id"].as_str().expect("an id").to_owned()
    };
    let card: Value = th
        .client
        .get(&format!("{at}/cards/{}", id_of("source")))
        .await
        .ok_json();
    assert_eq!(
        views_in(&card["body"]),
        [plan.id.as_str()],
        "{}",
        card["body"]
    );
    assert_eq!(card["body"]["content"][2], json!({"type": "paragraph"}));
    let reference = &card["body"]["content"][0]["content"][1];
    assert_eq!(
        reference["attrs"]["id"],
        json!(id_of("beside")),
        "{}",
        card["body"]
    );
    assert!(!card.to_string().contains(&other_view.id) && !card.to_string().contains(HIDDEN));
    th.teardown().await;
}

/// The Markdown of a body names a board view as a link to its view, by its board's title
/// and its own where the reader reads them, and as a block it does not hold as it is: a
/// body with one is not written whole from Markdown, and is added to.
#[tokio::test]
async fn markdown_names_a_board_view_and_keeps_it() {
    let th = Setup::native().start().await.init_basic().await;
    let open = made(&th.client, TEST_WS, "open", "Roadmap").await;
    let private = made(&th.client, TEST_WS, "private", HIDDEN).await;
    let plan = add_view(&th.client, &open.board, "Plan").await;
    let hidden = add_view(&th.client, &private.board, HIDDEN).await;
    let body = body_of("before", &[board_view(&plan.id), board_view(&hidden.id)]);
    let card_path = format!("{TEST_WS}/cards/{}", open.card);
    write_at(&th.client, &card_path, &body).await.check_ok();

    let read = |client: &Client| {
        let path = format!("{card_path}?format=markdown");
        let client = client.clone();
        async move { client.get(&path).await.ok_json::<Value>() }
    };
    let owner = read(&th.client).await;
    let markdown = owner["bodyMarkdown"].as_str().expect("Markdown");
    assert_eq!(
        markdown,
        format!(
            "before\n\n[Roadmap (its container) / Plan](fvoci-view://{})\n\n\
             [{HIDDEN} (its container) / {HIDDEN}](fvoci-view://{})",
            plan.id, hidden.id
        ),
    );
    assert_eq!(
        owner["complexBlocks"],
        json!([{"path": "/content/1", "reason": "board_view"},
            {"path": "/content/2", "reason": "board_view"}]),
    );
    let member = read(&th.client2).await;
    let markdown = member["bodyMarkdown"].as_str().expect("Markdown");
    assert!(
        markdown.ends_with(&format!("[Board view](fvoci-view://{})", hidden.id)),
        "{markdown}"
    );
    assert!(!markdown.contains(HIDDEN), "{markdown}");

    let whole = th
        .client
        .patch(
            &card_path,
            &json!({"bodyMarkdown": "after", "readAt": owner["updatedAt"]}),
        )
        .await;
    whole.check_status(reqwest::StatusCode::CONFLICT);
    assert_eq!(
        whole.error_reason(),
        "complex_blocks_use_append",
        "{}",
        whole.text()
    );
    th.client
        .post(
            &format!("{card_path}/body/markdown"),
            &json!({"markdown": "after"}),
        )
        .await
        .check_ok();
    let card = th.client.get(&card_path).await.value();
    assert_eq!(views_in(&card["body"]), [plan.id.as_str(), &hidden.id]);
    // What Markdown names by a view's link is read back as a line, not as a board view.
    th.client
        .post(
            &format!("{card_path}/body/markdown"),
            &json!({"markdown": format!("[again](fvoci-view://{})", plan.id)}),
        )
        .await
        .check_ok();
    let card = th.client.get(&card_path).await.value();
    assert_eq!(views_in(&card["body"]), [plan.id.as_str(), &hidden.id]);
    th.teardown().await;
}
