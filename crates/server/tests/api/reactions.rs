//! Reactions: who may comment answers a comment, a card, a page or a unit with an emoji
//! and takes it off again; the members read them with the comments and the card, page
//! and unit, and are told of them; nothing of the block changes, a diary is not there,
//! a token that writes reacts, and a reader by a public link is told nothing.

use std::fmt::Write as _;

use fvoci_model::Block;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::blocks::{boards_with_roles, card_with_content};
use crate::common::{ApiResponse, Client, ROOT_WORKSPACE_ID, Setup, TEST_WS, UNKNOWN_ID};
use crate::websocket::{
    Event, PATIENCE, QUIET, Socket, shared_board_with_a_card, subscribe_to_workspace,
};

/// 👍, as the path carries it.
const THUMBS_UP: &str = "%F0%9F%91%8D";
/// 🎉, as the path carries it.
const PARTY: &str = "%F0%9F%8E%89";

/// The path of the reaction `emoji` to `block_id` of `container_id`, in `ws`.
fn reaction_in(ws: &str, container_id: &str, block_id: &str, emoji: &str) -> String {
    format!("{ws}/containers/{container_id}/blocks/{block_id}/reactions/{emoji}")
}

/// The path of the reaction `emoji` to `block`, in the test workspace.
fn reaction(block: &Block, emoji: &str) -> String {
    reaction_in(TEST_WS, &block.container_id, &block.id, emoji)
}

async fn put(client: &Client, path: &str) -> ApiResponse {
    Client::send(client.request(Method::PUT, path)).await
}

/// The reactions of a read, `null` where it has none.
fn reactions_of(read: &Value) -> Value {
    read.get("reactions").cloned().unwrap_or(Value::Null)
}

/// The comments of the card, as `client` lists them.
async fn comments_of(client: &Client, card: &Block) -> Vec<Value> {
    client
        .get(&format!(
            "{TEST_WS}/boards/{}/cards/{}/comments",
            card.container_id, card.id
        ))
        .await
        .ok_json()
}

/// A commenter and up reacts; again is nothing more; the comments and the card are read
/// with their reactions, who reacted first first; one is taken off, and again is no
/// error. A viewer is refused; an emoji that is none, a block of no reacted type and a
/// block of another board are not taken.
#[tokio::test]
async fn whoever_may_comment_reacts_and_the_reactions_are_read_with_the_block() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let (commenter, editor) = (boards.commenter.me().await, boards.editor.me().await);
    let on_comment = reaction(&content.comment, THUMBS_UP);

    put(&boards.commenter, &on_comment).await.check_no_content();
    put(&boards.commenter, &on_comment).await.check_no_content();
    put(&boards.editor, &on_comment).await.check_no_content();
    put(&boards.viewer, &on_comment).await.check_forbidden();
    put(&th.client, &reaction(&content.card, PARTY))
        .await
        .check_no_content();

    let comments = comments_of(&th.client, &content.card).await;
    assert_eq!(
        reactions_of(&comments[0]),
        json!([{"emoji": "👍", "userIds": [commenter.id, editor.id]}]),
        "{comments:?}"
    );
    let card: Value = th
        .client
        .get(&format!("{TEST_WS}/cards/{}", content.card.id))
        .await
        .ok_json();
    let me = th.client.me().await;
    assert_eq!(
        reactions_of(&card),
        json!([{"emoji": "🎉", "userIds": [me.id]}])
    );

    boards
        .commenter
        .delete(&on_comment)
        .await
        .check_no_content();
    boards
        .commenter
        .delete(&on_comment)
        .await
        .check_no_content();
    let comments = comments_of(&th.client, &content.card).await;
    assert_eq!(
        reactions_of(&comments[0]),
        json!([{"emoji": "👍", "userIds": [editor.id]}])
    );
    boards.editor.delete(&on_comment).await.check_no_content();
    let comments = comments_of(&th.client, &content.card).await;
    assert_eq!(reactions_of(&comments[0]), Value::Null, "none is left out");

    let refused = put(&boards.commenter, &reaction(&content.comment, "ok")).await;
    refused.check_bad_request();
    for path in [
        reaction(&content.text, THUMBS_UP),
        reaction_in(
            TEST_WS,
            &content.card.container_id,
            &content.other_card.id,
            THUMBS_UP,
        ),
        reaction_in(TEST_WS, &content.card.container_id, UNKNOWN_ID, THUMBS_UP),
    ] {
        put(&th.client, &path).await.check_not_found();
    }
    th.teardown().await;
}

/// A reaction is no change of the block: its `updatedAt` and who changed it last stay.
#[tokio::test]
async fn a_reaction_changes_nothing_of_the_block() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let card_path = format!("{TEST_WS}/cards/{}", content.card.id);
    let changed = |read: &Value| (read["updatedAt"].clone(), read["modifiedBy"].clone());
    let card_before: Value = th.client.get(&card_path).await.ok_json();
    let comment_before = comments_of(&th.client, &content.card).await;

    put(&boards.editor, &reaction(&content.card, THUMBS_UP))
        .await
        .check_no_content();
    put(&boards.editor, &reaction(&content.comment, THUMBS_UP))
        .await
        .check_no_content();
    boards
        .editor
        .delete(&reaction(&content.card, THUMBS_UP))
        .await
        .check_no_content();

    let card_after: Value = th.client.get(&card_path).await.ok_json();
    let comment_after = comments_of(&th.client, &content.card).await;
    assert_eq!(changed(&card_after), changed(&card_before), "the card");
    assert_eq!(
        changed(&comment_after[0]),
        changed(&comment_before[0]),
        "the comment"
    );
    th.teardown().await;
}

/// A block has twenty different emoji at most: one more is 400 with its reason and the
/// limit; one of the twenty is taken.
#[tokio::test]
async fn a_block_takes_twenty_emoji_and_no_more() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let animals: Vec<String> = ('\u{1F400}'..='\u{1F414}')
        .map(|animal| urlencoding(&animal.to_string()))
        .collect();
    for animal in &animals[..20] {
        put(&th.client, &reaction(&content.card, animal))
            .await
            .check_no_content();
    }
    let refused = put(&boards.editor, &reaction(&content.card, &animals[20])).await;
    refused.check_bad_request();
    let problem = refused.value();
    assert_eq!(
        (&problem["reason"], &problem["limit"]),
        (&json!("too_many_reactions"), &json!(20)),
        "{problem}"
    );
    put(&boards.editor, &reaction(&content.card, &animals[0]))
        .await
        .check_no_content();
    th.teardown().await;
}

/// The text as a path carries it: every byte of its UTF-8 percent-encoded.
fn urlencoding(text: &str) -> String {
    text.bytes().fold(String::new(), |mut encoded, byte| {
        // Writing to a string cannot fail.
        let _ = write!(encoded, "%{byte:02X}");
        encoded
    })
}

/// A page and its comment, a unit and its comment take reactions, and are read with
/// them.
#[tokio::test]
async fn pages_and_units_and_their_comments_take_reactions() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await;
    let said = json!({"body": {"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "noted"}]},
    ]}});
    let wiki = crate::wiki_copies::wiki(&th.client, "a wiki").await;
    let page = crate::wiki_copies::page(&th.client, &wiki, "", "a page").await;
    let made = th
        .client
        .post(
            &format!("{TEST_WS}/wikis/{wiki}/pages/{page}/comments"),
            &said,
        )
        .await;
    let page_comment: Value = made.created_json();
    let course: Value = th
        .client
        .post(
            &format!("{TEST_WS}/courses"),
            &json!({"visibility": "open", "title": "a course"}),
        )
        .await
        .created_json();
    let course = course["id"].as_str().expect("a course").to_owned();
    let unit: Value = th
        .client
        .post(
            &format!("{TEST_WS}/courses/{course}/units"),
            &json!({"title": "a unit"}),
        )
        .await
        .created_json();
    let unit = unit["id"].as_str().expect("a unit").to_owned();
    let made = th
        .client
        .post(
            &format!("{TEST_WS}/courses/{course}/units/{unit}/comments"),
            &said,
        )
        .await;
    let unit_comment: Value = made.created_json();

    let page_comment_id = page_comment["id"].as_str().expect("a comment");
    let unit_comment_id = unit_comment["id"].as_str().expect("a comment");
    for (container, block) in [
        (&wiki, page.as_str()),
        (&wiki, page_comment_id),
        (&course, unit.as_str()),
        (&course, unit_comment_id),
    ] {
        put(
            &th.client,
            &reaction_in(TEST_WS, container, block, THUMBS_UP),
        )
        .await
        .check_no_content();
    }

    let mine = json!([{"emoji": "👍", "userIds": [me.id]}]);
    for (read, comments) in [
        (
            format!("{TEST_WS}/wikis/{wiki}/pages/{page}"),
            format!("{TEST_WS}/wikis/{wiki}/pages/{page}/comments"),
        ),
        (
            format!("{TEST_WS}/courses/{course}/units/{unit}"),
            format!("{TEST_WS}/courses/{course}/units/{unit}/comments"),
        ),
    ] {
        let block: Value = th.client.get(&read).await.ok_json();
        assert_eq!(reactions_of(&block), mine, "{read}");
        let comments: Vec<Value> = th.client.get(&comments).await.ok_json();
        assert_eq!(reactions_of(&comments[0]), mine, "{comments:?}");
    }
    th.teardown().await;
}

/// A diary's entry is its owner's alone: its diary is not there to a reaction, as a
/// container nobody has, to add one or take one off, by the owner or by their token
/// that writes and was given the space.
#[tokio::test]
async fn a_diary_is_not_there_to_reactions() {
    let crate::api_tokens_diaries::Space {
        th,
        ws,
        diary_id,
        entry,
        agent,
        ..
    } = crate::api_tokens_diaries::a_space_with_a_diary().await;
    let entry_id = crate::diaries::last_of(&entry);
    for (who, client) in [("the owner", &th.client), ("their token", &agent)] {
        for method in [Method::PUT, Method::DELETE] {
            let ask = async |container: &str| {
                let path = reaction_in(&ws, container, entry_id, THUMBS_UP);
                Client::send(client.request(method.clone(), &path)).await
            };
            let (there, nothing) = (ask(&diary_id).await, ask(UNKNOWN_ID).await);
            assert_eq!(
                (there.status, there.error_message()),
                (nothing.status, nothing.error_message()),
                "{who}: {method}"
            );
            assert_eq!(there.status, StatusCode::NOT_FOUND, "{who}: {method}");
        }
    }
    th.teardown().await;
}

/// A personal API token that writes reacts for its user; one that reads does not.
#[tokio::test]
async fn a_token_that_writes_reacts() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let agent = crate::api_tokens::agent_writing(&th, &th.client).await;
    put(&agent, &reaction(&content.card, THUMBS_UP))
        .await
        .check_no_content();
    let me = th.client.me().await;
    let card: Value = th
        .client
        .get(&format!("{TEST_WS}/cards/{}", content.card.id))
        .await
        .ok_json();
    assert_eq!(
        reactions_of(&card),
        json!([{"emoji": "👍", "userIds": [me.id]}])
    );

    let (board, card, _) = shared_board_with_a_card(&th, &me.id).await;
    let reader = crate::api_tokens::agent_reading(&th, &th.client).await;
    let path = reaction_in(
        &format!("/w/{ROOT_WORKSPACE_ID}"),
        &board.id,
        &card.id,
        THUMBS_UP,
    );
    let refused = put(&reader, &path).await;
    refused.check_forbidden();
    assert_eq!(refused.value()["reason"], json!("token_scope_insufficient"));
    th.teardown().await;
}

/// The members who follow the workspace are told the block's reactions as they are now,
/// once for each change and not for one that changes nothing; a reader of the board by
/// its link, who follows the block, is told nothing.
#[tokio::test]
async fn the_members_are_told_and_a_reader_by_the_link_is_not() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let me = th.client.me().await;
    let (board, card, token) = shared_board_with_a_card(&th, &me.id).await;
    let mut member = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut member, ROOT_WORKSPACE_ID, &me.id).await;
    let mut reader = Socket::connect(&th).await;
    reader
        .send(json!({
            "type": "subscribeBlocks",
            "workspaceId": ROOT_WORKSPACE_ID,
            "readToken": token,
            "blockIds": [card.id],
        }))
        .await;
    let ack = reader.next_json(PATIENCE).await.expect("subscribed");
    assert_eq!(ack["type"], json!("blocksSubscribed"), "{ack}");
    let path = reaction_in(
        &format!("/w/{ROOT_WORKSPACE_ID}"),
        &board.id,
        &card.id,
        THUMBS_UP,
    );

    put(&th.client, &path).await.check_no_content();
    let told = member
        .next_json(PATIENCE)
        .await
        .expect("the member is told");
    assert_eq!(
        told,
        json!({
            "type": "reactionsChanged",
            "workspaceId": ROOT_WORKSPACE_ID,
            "containerId": board.id,
            "blockId": card.id,
            "reactions": [{"emoji": "👍", "userIds": [me.id]}],
        })
    );
    put(&th.client, &path).await.check_no_content();
    assert_eq!(member.next(QUIET).await, Event::Nothing, "nothing changed");
    th.client.delete(&path).await.check_no_content();
    let told = member
        .next_json(PATIENCE)
        .await
        .expect("the member is told");
    assert_eq!(told["reactions"], json!([]), "{told}");
    assert_eq!(
        reader.next(QUIET).await,
        Event::Nothing,
        "the reader is not"
    );
    th.teardown().await;
}
