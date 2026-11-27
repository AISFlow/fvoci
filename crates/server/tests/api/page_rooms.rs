//! The room of a page's body: a page has one as a card has, entered by the page's
//! identifier, for who its wiki lets in and as its wiki lets them.

use std::time::Duration;

use fvoci_model::Page;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::presence::{people, visit};
use crate::rooms::{PATIENCE, Peer, QUIET, give, lines_of, paragraphs, table};
use crate::websocket::Socket;

/// A wiki for the invited only in the test workspace, made by the first client's
/// user, with a page.
pub(crate) struct Desk {
    pub(crate) wiki_id: String,
    pub(crate) page_id: String,
}

impl Desk {
    fn pages(&self) -> String {
        format!("{TEST_WS}/wikis/{}/pages", self.wiki_id)
    }

    fn page(&self) -> String {
        format!("{}/{}", self.pages(), self.page_id)
    }
}

/// A [`Desk`] whose page has the body `body`.
pub(crate) async fn desk(th: &TestServer, body: &Value) -> Desk {
    let wiki = th
        .client
        .post(
            &format!("{TEST_WS}/wikis"),
            &json!({"visibility": "private"}),
        )
        .await;
    wiki.check_created();
    let wiki_id = wiki.value()["id"].as_str().expect("an id").to_owned();
    let pages = format!("{TEST_WS}/wikis/{wiki_id}/pages");
    let page = written(
        &th.client,
        &pages,
        json!({"title": "Edited together"}),
        body,
    )
    .await;
    Desk {
        wiki_id,
        page_id: page.id,
    }
}

/// Makes the page `page` among `pages` and writes it the body `body`: a page is made
/// with none.
async fn written(client: &Client, pages: &str, page: Value, body: &Value) -> Page {
    let made: Page = client.post(pages, &page).await.created_json();
    client
        .patch(
            &format!("{pages}/{}", made.id),
            &json!({"body": body, "readAt": made.updated_at}),
        )
        .await
        .ok_json()
}

/// The two clients' users in the room of a page with `body`, both writing: the second
/// is given the editor's role on the wiki.
async fn two_at_a_desk(th: &TestServer, body: &Value) -> (Desk, Peer, Peer) {
    let d = desk(th, body).await;
    let user2 = th.client2.me().await.id;
    give(th, &d.wiki_id, &user2, "editor").await;
    let mut one = Peer::connect(&th.client, 101).await;
    let mut two = Peer::connect(&th.client2, 102).await;
    one.joined(TEST_WORKSPACE_ID, &d.page_id).await;
    two.joined(TEST_WORKSPACE_ID, &d.page_id).await;
    (d, one, two)
}

async fn read(client: &Client, path: &str) -> Page {
    client.get(path).await.ok_json()
}

fn body_of(page: &Page) -> Option<Value> {
    page.body.as_ref().map(|body| body.document().clone())
}

/// Waits until the body of the page at `path` is `body`.
async fn page_holds(client: &Client, path: &str, body: &Value) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let held = body_of(&read(client, path).await);
        if held.as_ref() == Some(body) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the page holds {held:?}, not {body}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Two users come into the room of a page and write, in turn and at once: both hold
/// the same document, and the page is given the body of both when they leave, which
/// is what a later reader of the page and a later comer to the room are given.
#[tokio::test]
async fn two_users_edit_a_page_together_and_converge() {
    let th = Setup::native().start().await.init_basic().await;
    let d = desk(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    give(&th, &d.wiki_id, &user2, "editor").await;

    let mut listener =
        WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &user2).await;
    let mut one = Peer::connect(&th.client, 101).await;
    let mut two = Peer::connect(&th.client2, 102).await;
    let joined = one.joined(TEST_WORKSPACE_ID, &d.page_id).await;
    assert_eq!(
        joined,
        json!({
            "type": "docJoined", "workspaceId": TEST_WORKSPACE_ID, "cardId": d.page_id,
            "doc": 1, "epoch": 1, "mode": "write",
        })
    );
    two.joined(TEST_WORKSPACE_ID, &d.page_id).await;
    assert_eq!(one.lines(), ["first"]);
    assert_eq!(two.lines(), ["first"]);

    // In turn: each has the other's before it writes.
    one.write_acked("one writes").await;
    two.settle().await;
    assert_eq!(two.lines(), ["first", "one writes"]);
    two.write_acked("two answers").await;
    one.settle().await;
    // At once: each writes before it has the other's.
    one.write("one at once").await;
    two.write("two at once").await;
    for peer in [&mut one, &mut two] {
        peer.text("docAck").await;
        peer.settle().await;
    }
    let merged = one.body().expect("a body");
    assert_eq!(two.body(), Some(merged.clone()), "both hold the same");
    assert_eq!(lines_of(Some(&merged)).len(), 5, "{merged}");

    // The last to leave closes the document, which writes its body to the page: by
    // whom of the two wrote last, for whoever reads the page.
    one.leave().await;
    two.leave().await;
    page_holds(&th.client2, &d.page(), &merged).await;
    let page = read(&th.client, &d.page()).await;
    let me = th.client.me().await.id;
    assert!(
        [&me, &user2].contains(&&page.modified_by),
        "written by one of the two: {}",
        page.modified_by
    );
    assert_eq!(page.title, "Edited together", "the body only is written");
    // The wiki's members are told of the page as it is written, as of a card: the
    // summary of its body, which is read over HTTP (above).
    let words = |told: &Value| {
        told["summary"]["preview"]["text"]
            .as_str()
            .map(str::to_owned)
    };
    let all = lines_of(Some(&merged)).join("\n");
    let mut told = listener
        .expect("blockChanged", "/block/id", &d.page_id)
        .await;
    while words(&told).as_deref() != Some(all.trim_end()) {
        told = listener
            .expect("blockChanged", "/block/id", &d.page_id)
            .await;
    }
    assert_eq!(
        (&told["block"]["type"], &told["block"]["containerId"]),
        (&json!("page"), &json!(d.wiki_id)),
        "{told}"
    );
    assert_eq!(told["block"]["body"], Value::Null, "{told}");

    // Who comes later is sent all of it.
    let mut three = Peer::connect(&th.client2, 103).await;
    three.joined(TEST_WORKSPACE_ID, &d.page_id).await;
    assert_eq!(three.body(), Some(merged));
    three.leave().await;
    th.teardown().await;
}

/// A page's body written whole over HTTP goes around its room, as a card's does: what
/// the room typed is written to the page first, so that a writer who read the page
/// before is refused, and the body that is written is sent to the room as another
/// document.
#[tokio::test]
async fn a_page_body_written_whole_goes_around_its_room() {
    let th = Setup::native().start().await.init_basic().await;
    let (d, mut one, mut two) = two_at_a_desk(&th, &paragraphs(&["first"])).await;
    let before = read(&th.client, &d.page()).await;
    one.write_acked("typed meanwhile").await;
    two.settle().await;

    let whole = paragraphs(&["written whole"]);
    let stale = th
        .client
        .patch(
            &d.page(),
            &json!({"body": whole, "readAt": before.updated_at}),
        )
        .await;
    assert_eq!(stale.status, StatusCode::CONFLICT, "{}", stale.text());
    let typed = read(&th.client, &d.page()).await;
    assert_eq!(
        lines_of(body_of(&typed).as_ref()),
        ["first", "typed meanwhile"],
        "what the room typed is in the page before a whole body is"
    );
    let written: Page = th
        .client
        .patch(
            &d.page(),
            &json!({"body": whole, "readAt": typed.updated_at}),
        )
        .await
        .ok_json();
    assert_eq!(body_of(&written), Some(whole.clone()));

    for peer in [&mut one, &mut two] {
        let reset = peer.text("docReset").await;
        assert_eq!(
            reset,
            json!({"type": "docReset", "doc": peer.doc, "epoch": 2})
        );
        peer.settle().await;
        assert_eq!(
            peer.lines(),
            ["written whole"],
            "the document before is gone"
        );
    }
    // The room goes on in the new document.
    one.write_acked("one goes on").await;
    two.settle().await;
    assert_eq!(two.lines(), ["written whole", "one goes on"]);
    let body = two.body().expect("a body");
    one.leave().await;
    two.leave().await;
    page_holds(&th.client, &d.page(), &body).await;
    th.teardown().await;
}

/// A version of the page restored while it is edited together, as a card's: what the
/// editors typed is kept as a version first, everyone in the room is sent the version
/// as a new document they go on in, and the wiki's members are told of the page.
#[tokio::test]
async fn a_version_of_a_page_restored_goes_around_its_room_and_its_wiki() {
    let th = Setup::native().start().await.init_basic().await;
    let (d, mut one, mut two) = two_at_a_desk(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    one.write_acked("typed").await;
    two.settle().await;
    let mut listener =
        WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &user2).await;
    let versions = format!("{}/body-versions", d.page());
    let kept: Vec<Value> = th.client.get(&versions).await.ok_json();
    let first = kept.last().expect("the body it was written with")["version"].clone();

    let restored: Page = th
        .client2
        .post_empty(&format!("{versions}/{first}/restore"))
        .await
        .ok_json();
    assert_eq!(lines_of(body_of(&restored).as_ref()), ["first"]);
    assert_eq!(restored.modified_by, user2, "written by who restored it");
    for peer in [&mut one, &mut two] {
        let reset = peer.text("docReset").await;
        assert_eq!(reset["type"], json!("docReset"));
        peer.settle().await;
        assert_eq!(
            peer.lines(),
            ["first"],
            "the version, in place of the document"
        );
    }
    let kept: Vec<Value> = th.client.get(&versions).await.ok_json();
    let mut typed_kept = false;
    for version in &kept {
        let number = &version["version"];
        let read: Value = th
            .client
            .get(&format!("{versions}/{number}"))
            .await
            .ok_json();
        typed_kept |= lines_of(read.get("body")) == ["first", "typed"];
    }
    assert!(typed_kept, "what was typed is kept: {kept:?}");
    let mut heard = Vec::new();
    for _ in 0..20 {
        heard.extend(listener.drain().await);
        if heard
            .iter()
            .any(|told| told["block"]["id"] == json!(d.page_id))
        {
            break;
        }
    }
    assert!(
        heard
            .iter()
            .any(|told| told["type"] == json!("blockChanged")
                && told["block"]["id"] == json!(d.page_id)),
        "the wiki's members are told of the page: {heard:?}"
    );
    one.leave().await;
    two.leave().await;
    th.teardown().await;
}

/// Who is in the room of a page is who its wiki lets read it, as the wiki lets them:
/// nobody else, a reader who writes nothing, and a writer; and who is taken off the
/// wiki is put out of the room.
#[tokio::test]
async fn the_room_of_a_page_is_for_whom_its_wiki_lets_in() {
    let th = Setup::native().start().await.init_basic().await;
    let d = desk(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    let mut one = Peer::connect(&th.client, 101).await;
    let mut two = Peer::connect(&th.client2, 102).await;
    one.joined(TEST_WORKSPACE_ID, &d.page_id).await;

    // Not on the wiki: as for no page. Nor is the wiki itself a page.
    let refused = json!({
        "type": "docRefused", "workspaceId": TEST_WORKSPACE_ID, "cardId": d.page_id,
        "reason": "not_found",
    });
    assert_eq!(two.join(TEST_WORKSPACE_ID, &d.page_id).await, refused);
    let no_page = one.join(TEST_WORKSPACE_ID, &d.wiki_id).await;
    assert_eq!(
        (&no_page["type"], &no_page["reason"]),
        (&json!("docRefused"), &json!("not_found")),
        "{no_page}"
    );

    // A viewer of the wiki reads, and what they send is refused.
    give(&th, &d.wiki_id, &user2, "viewer").await;
    let joined = two.joined(TEST_WORKSPACE_ID, &d.page_id).await;
    assert_eq!(joined["mode"], json!("read"), "{joined}");
    two.write("a reader writes").await;
    let nack = two.text("docNack").await;
    assert_eq!(nack["reason"], json!("document_read_only"), "{nack}");
    // The reader holds a paragraph the room refused: it comes in anew.
    two.joined(TEST_WORKSPACE_ID, &d.page_id).await;

    // Made an editor, they are told so while they are there, and write.
    give(&th, &d.wiki_id, &user2, "editor").await;
    let mode = two.text("docModeChanged").await;
    assert_eq!(
        mode,
        json!({"type": "docModeChanged", "doc": two.doc, "mode": "write"})
    );
    two.write_acked("an editor writes").await;
    one.settle().await;
    assert_eq!(one.lines(), ["first", "an editor writes"]);

    // Taken off the wiki, they are put out; the other stays.
    th.client
        .delete(&format!(
            "{TEST_WS}/containers/{}/members/{user2}",
            d.wiki_id
        ))
        .await
        .check_no_content();
    let left = two.text("docLeft").await;
    assert_eq!(
        left,
        json!({"type": "docLeft", "doc": two.doc, "reason": "not_found"})
    );
    assert_eq!(two.join(TEST_WORKSPACE_ID, &d.page_id).await, refused);
    one.write_acked("the other stays").await;
    th.teardown().await;
}

/// A page that is deleted, with a page above it or with its wiki, has no room any
/// more: who was in it is put out, and what they typed last is written to the page
/// before it goes, so that the trash keeps it. A wiki that comes back has the page
/// with that body, for who reads it and for who comes into its room.
#[tokio::test]
async fn deleting_a_page_or_its_wiki_closes_the_rooms_of_what_goes_with_it() {
    for target in ["page", "page above", "wiki"] {
        let th = Setup::native().start().await.init_basic().await;
        let d = desk(&th, &paragraphs(&["above"])).await;
        let below = json!({"title": "Below", "parentId": d.page_id});
        let below = written(&th.client, &d.pages(), below, &paragraphs(&["first"])).await;
        let below_at = format!("{}/{}", d.pages(), below.id);
        let mut writer = Peer::connect(&th.client, 101).await;
        writer.joined(TEST_WORKSPACE_ID, &below.id).await;
        writer.write_acked("the last edit").await;
        let wanted = writer.body();
        let stored = body_of(&read(&th.client, &below_at).await);
        assert_ne!(stored, wanted, "the room has not written the page yet");

        let wiki = format!("{TEST_WS}/containers/{}", d.wiki_id);
        let gone = match target {
            "page" => below_at.clone(),
            "page above" => d.page(),
            "wiki" => wiki.clone(),
            other => unreachable!("no such target: {other}"),
        };
        th.client.delete(&gone).await.check_no_content();
        let left = writer.text("docLeft").await;
        assert_eq!(left["reason"], json!("not_found"), "{target}: {left}");
        assert_eq!(th.app.collab().rooms(), 0, "{target}");
        let again = writer.join(TEST_WORKSPACE_ID, &below.id).await;
        assert_eq!(again["type"], json!("docRefused"), "{target}: {again}");
        // Written by the deletion, not by a room that opens later.
        let trashed = th.app.store().get_deleted_block(&below.id).await;
        let kept = trashed.expect("in the trash").block.body;
        assert_eq!(
            kept.map(|body| body.document().clone()),
            wanted,
            "{target}: the trash keeps the last edit"
        );
        if target == "wiki" {
            th.client
                .post_empty(&format!("{wiki}/restore"))
                .await
                .check_ok();
            let back = body_of(&read(&th.client, &below_at).await);
            assert_eq!(back, wanted, "the page comes back as it went");
            let mut reopened = Peer::connect(&th.client, 203).await;
            reopened.joined(TEST_WORKSPACE_ID, &below.id).await;
            assert_eq!(reopened.body(), wanted, "and its room with it");
        }
        th.teardown().await;
    }
}

/// Who visits a wiki is shown to its other visitors with the page they have open, as
/// who visits a board is with their card; a card of a board is no open page of the
/// wiki.
#[tokio::test]
async fn an_open_page_is_named_in_the_presence_of_its_wiki() {
    let th = Setup::native().start().await.init_basic().await;
    let d = desk(&th, &paragraphs(&["hello"])).await;
    let elsewhere = table(&th, &paragraphs(&["a card"])).await;
    let other = th.client2.me().await.id;
    give(&th, &d.wiki_id, &other, "viewer").await;
    let mut one = Socket::connect_as(&th.client).await;
    visit(&mut one, &d.wiki_id, "").await;
    let _ = people(&mut one, 1).await;
    let mut two = Socket::connect_as(&th.client2).await;

    let open_of_other = |shown: &Value| {
        let people = shown["people"].as_array().expect("people");
        let person = people.iter().find(|person| person["userId"] == other);
        person.expect("the other reader")["cardIds"].clone()
    };
    visit(&mut two, &d.wiki_id, &d.page_id).await;
    let shown = people(&mut one, 2).await;
    assert_eq!(
        open_of_other(&shown),
        json!([d.page_id]),
        "the open page is named"
    );
    visit(&mut two, &d.wiki_id, &elsewhere.card_id).await;
    let shown = people(&mut one, 2).await;
    assert_eq!(
        open_of_other(&shown),
        json!([]),
        "a card of a board is no page of the wiki"
    );
    // A page that is deleted is open no more for who visits again: on the wiki still.
    visit(&mut two, &d.wiki_id, &d.page_id).await;
    let _ = people(&mut one, 2).await;
    th.client.delete(&d.page()).await.check_no_content();
    visit(&mut two, &d.wiki_id, "").await;
    let _ = people(&mut one, 2).await;
    visit(&mut two, &d.wiki_id, &d.page_id).await;
    assert!(
        one.next_json(QUIET).await.is_none(),
        "a deleted page is no open page: the roster stays as it was"
    );
    th.teardown().await;
}
