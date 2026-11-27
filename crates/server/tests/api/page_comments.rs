//! The comments on a page of a wiki: what each role of the wiki does to them, as to a
//! card's comments; that a comment is reached through its own page only; what a request
//! of one is held to; who is told of one; and that they go with their page.

use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::common::{Client, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS, UNKNOWN_ID};
use crate::wiki_isolation::{put_on, wiki_of};

/// A text of a comment: one paragraph.
fn text(said: &str) -> Value {
    json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": said}
    ]}]})
}

/// Where the comments of the page are.
fn comments_of(at: &str, wiki: &str, page: &str) -> String {
    format!("{at}/wikis/{wiki}/pages/{page}/comments")
}

/// Writes a comment on the page as `client`: its block.
async fn written(client: &Client, wiki: &str, page: &str, said: &str) -> Value {
    let made = client
        .post(
            &comments_of(TEST_WS, wiki, page),
            &json!({"body": text(said)}),
        )
        .await;
    made.check_created();
    made.value()
}

fn id(value: &Value) -> String {
    value["id"].as_str().expect("an id").to_owned()
}

/// What the operations of comments answer about the comment `theirs` of somebody else,
/// in this order: the list, a new comment, the versions of theirs, a change of theirs,
/// the deletion of theirs.
type OnTheirs = [u16; 5];

async fn on_theirs(client: &Client, at: &str, wiki: &str, page: &str, theirs: &Value) -> OnTheirs {
    let comments = comments_of(at, wiki, page);
    let comment = format!("{comments}/{}", id(theirs));
    let change = json!({"body": text("changed"), "readAt": theirs["updatedAt"]});
    [
        client.get(&comments).await,
        client.post(&comments, &json!({"body": text("mine")})).await,
        client.get(&format!("{comment}/versions")).await,
        client.patch(&comment, &change).await,
        client.delete(&comment).await,
    ]
    .map(|response| response.status.as_u16())
}

/// What the operations of comments answer about a comment of `client`'s own, in this
/// order: a change, the versions, the deletion; and whether the versions had the text
/// it was written with.
async fn on_their_own(client: &Client, wiki: &str, page: &str) -> ([u16; 3], bool) {
    let mine = written(client, wiki, page, "first").await;
    let comment = format!("{}/{}", comments_of(TEST_WS, wiki, page), id(&mine));
    let change = json!({"body": text("second"), "readAt": mine["updatedAt"]});
    let changed = client.patch(&comment, &change).await;
    if changed.status.is_success() {
        assert!(
            changed.value()["editedAt"].as_i64().unwrap_or(0) > 0,
            "{}",
            changed.text()
        );
    }
    let versions = client.get(&format!("{comment}/versions")).await;
    let kept_first = versions.status.is_success() && versions.value()[0]["body"] == text("first");
    let deleted = client.delete(&comment).await;
    (
        [changed, versions, deleted].map(|response| response.status.as_u16()),
        kept_first,
    )
}

/// A viewer reads the comments, a commenter writes their own and changes and deletes
/// them, an editor no more than that, and the comment of somebody else is the wiki's
/// admins' alone to change or delete: as the comments of a card are.
#[tokio::test]
async fn each_role_of_the_wiki_does_to_the_comments_of_a_page_what_it_does_to_a_cards() {
    let th = Setup::native().start().await.init_basic().await;
    let member = th.client2.me().await.id;
    let (wiki, page) = wiki_of(&th.client, "private").await;
    let theirs = written(&th.client, &wiki, &page, "the admin's").await;
    // Changed once, so that its versions are there to read.
    let at = format!("{}/{}", comments_of(TEST_WS, &wiki, &page), id(&theirs));
    let changed = th
        .client
        .patch(
            &at,
            &json!({"body": text("again"), "readAt": theirs["updatedAt"]}),
        )
        .await;
    changed.check_ok();
    let theirs = changed.value();

    for (role, expected) in [
        ("viewer", [200, 403, 200, 403, 403]),
        ("commenter", [200, 201, 200, 403, 403]),
        ("editor", [200, 201, 200, 403, 403]),
    ] {
        put_on(&th.client, &wiki, &member, role).await;
        assert_eq!(
            on_theirs(&th.client2, TEST_WS, &wiki, &page, &theirs).await,
            expected,
            "{role}"
        );
        if role != "viewer" {
            assert_eq!(
                on_their_own(&th.client2, &wiki, &page).await,
                ([200, 200, 204], true),
                "{role}"
            );
        }
    }
    // The comment of somebody else, by the admin: changed and deleted.
    let mine = written(&th.client2, &wiki, &page, "the member's").await;
    let comments = comments_of(TEST_WS, &wiki, &page);
    let listed = th.client.get(&comments).await.value();
    let listed: Vec<String> = listed.as_array().expect("a list").iter().map(id).collect();
    assert!(listed.ends_with(&[id(&mine)]), "oldest first: {listed:?}");
    let at = format!("{comments}/{}", id(&mine));
    th.client
        .patch(
            &at,
            &json!({"body": text("moderated"), "readAt": mine["updatedAt"]}),
        )
        .await
        .check_ok();
    th.client.delete(&at).await.check_no_content();
    th.teardown().await;
}

/// Who is not on the wiki, and who names it under another workspace, is answered as for
/// a wiki that is nowhere.
#[tokio::test]
async fn who_may_not_read_the_wiki_is_answered_of_its_comments_as_of_none() {
    let th = Setup::native().start().await.init_basic().await;
    let (wiki, page) = wiki_of(&th.client, "private").await;
    let theirs = written(&th.client, &wiki, &page, "hidden").await;
    let nothing = json!({"id": UNKNOWN_ID, "updatedAt": 1});
    let none = on_theirs(&th.client2, TEST_WS, UNKNOWN_ID, UNKNOWN_ID, &nothing).await;
    assert_eq!(none, [404; 5]);
    for (client, at) in [(&th.client2, TEST_WS), (&th.client, ROOT_WS)] {
        assert_eq!(
            on_theirs(client, at, &wiki, &page, &theirs).await,
            none,
            "{at}"
        );
        let of_wiki = client.get(&comments_of(at, &wiki, &page)).await;
        let of_none = client.get(&comments_of(at, UNKNOWN_ID, &page)).await;
        assert_eq!(of_wiki.problem(), of_none.problem(), "{at}");
    }
    th.teardown().await;
}

/// Who works in the workspace and did not join an open wiki reads its comments and
/// what they said before, as its viewers do, and writes none: not even, once they
/// left it, a change, deletion or restoring of a comment they wrote while they were in.
#[tokio::test]
async fn an_open_wiki_not_joined_has_its_comments_read_as_a_viewer_does() {
    let th = Setup::native().start().await.init_basic().await;
    let (open, page) = wiki_of(&th.client, "open").await;
    let theirs = written(&th.client, &open, &page, "open").await;
    assert_eq!(
        on_theirs(&th.client2, TEST_WS, &open, &page, &theirs).await,
        [200, 403, 200, 403, 403]
    );
    let read = th
        .client2
        .get(&comments_of(TEST_WS, &open, &page))
        .await
        .value();
    let read: Vec<String> = read.as_array().expect("a list").iter().map(id).collect();
    assert_eq!(
        read,
        [id(&theirs)],
        "what who did not join the open wiki reads"
    );

    let membership = format!("{TEST_WS}/containers/{open}");
    th.client2
        .post(&format!("{membership}/join"), &json!({}))
        .await
        .check_ok();
    let kept = written(&th.client2, &open, &page, "kept").await;
    let gone = written(&th.client2, &open, &page, "gone").await;
    let comments = comments_of(TEST_WS, &open, &page);
    th.client2
        .delete(&format!("{comments}/{}", id(&gone)))
        .await
        .check_no_content();
    th.client2
        .post(&format!("{membership}/leave"), &json!({}))
        .await
        .check_no_content();

    let own = format!("{comments}/{}", id(&kept));
    let change = json!({"body": text("changed"), "readAt": kept["updatedAt"]});
    let answers = [
        th.client2.patch(&own, &change).await,
        th.client2.delete(&own).await,
        th.client2
            .post(&format!("{comments}/{}/restore", id(&gone)), &json!({}))
            .await,
    ]
    .map(|response| response.status.as_u16());
    assert_eq!(answers, [403; 3], "their own, once they left");
    th.teardown().await;
}

/// A comment is reached through the page it is on: not through another page of its
/// wiki, a page of another wiki, nor as a page below the page; and a card's comment is
/// no page's.
#[tokio::test]
async fn a_comment_is_reached_through_its_own_page_only() {
    let th = Setup::native().start().await.init_basic().await;
    let client = &th.client;
    let (wiki, page) = wiki_of(client, "private").await;
    let (other_wiki, other_page) = wiki_of(client, "private").await;
    let sibling = client
        .post(
            &format!("{TEST_WS}/wikis/{wiki}/pages"),
            &json!({"title": "sibling"}),
        )
        .await;
    sibling.check_created();
    let sibling = id(&sibling.value());
    let child = client
        .post(
            &format!("{TEST_WS}/wikis/{wiki}/pages"),
            &json!({"title": "below", "parentId": page}),
        )
        .await;
    child.check_created();
    let child = json!({"id": id(&child.value()), "updatedAt": 1});
    let comment = written(client, &wiki, &page, "here").await;
    let board = client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"visibility": "private"}),
        )
        .await;
    board.check_created();
    let board = id(&board.value());
    let card = client
        .post(
            &format!("{TEST_WS}/boards/{board}/cards"),
            &json!({"title": "a card"}),
        )
        .await;
    card.check_created();
    let card = id(&card.value());
    let on_card = client
        .post(
            &format!("{TEST_WS}/boards/{board}/blocks"),
            &json!([{"containerId": board, "parentId": card, "type": "comment",
                     "body": text("on a card"), "createdAt": 1, "updatedAt": 1}]),
        )
        .await;
    on_card.check_created();
    let on_card = on_card.value()[0].clone();

    let nothing = json!({"id": UNKNOWN_ID, "updatedAt": 1});
    let none = on_theirs(client, TEST_WS, &wiki, &page, &nothing).await;
    assert_eq!(none, [200, 201, 404, 404, 404]);
    for (wiki, page, comment, what) in [
        (&wiki, &sibling, &comment, "on another page"),
        (&other_wiki, &other_page, &comment, "under another wiki"),
        (&wiki, &page, &child, "a page below"),
        (&wiki, &page, &on_card, "a card's"),
    ] {
        assert_eq!(
            on_theirs(client, TEST_WS, wiki, page, comment).await,
            none,
            "{what}"
        );
    }
    // Nor is the page of another wiki one to comment on, nor a card a page.
    for (wiki, page) in [(&wiki, &other_page), (&board, &card)] {
        let made = client
            .post(
                &comments_of(TEST_WS, wiki, page),
                &json!({"body": text("stray")}),
            )
            .await;
        made.check_not_found();
    }
    // Nor is a page's comment one of a board's blocks, under a board the user has.
    let blocks = format!("{TEST_WS}/boards/{board}/blocks");
    let block = format!("{blocks}/{}", id(&comment));
    let patched = json!({"blockIds": [id(&comment)], "blockPatches": [{"title": "x"}]});
    for refused in [
        client.patch(&block, &json!({"title": "x"})).await,
        client.patch(&blocks, &patched).await,
        client.delete(&block).await,
        client.get(&format!("{block}/versions")).await,
        client.post(&format!("{block}/restore"), &json!({})).await,
        client
            .get(&format!("{blocks}?blockId={}", id(&comment)))
            .await,
    ] {
        refused.check_not_found();
    }
    // The comment is still there, as it was.
    let listed = client
        .get(&comments_of(TEST_WS, &wiki, &page))
        .await
        .value();
    assert_eq!(listed[0]["body"], text("here"));
    th.teardown().await;
}

/// A comment has a text, is changed over what was read of it, and is a comment and no
/// more: the request names nothing else of it.
#[tokio::test]
async fn a_comment_is_a_text_written_over_what_was_read() {
    let th = Setup::native().start().await.init_basic().await;
    let client = &th.client;
    let (wiki, page) = wiki_of(client, "private").await;
    let comments = comments_of(TEST_WS, &wiki, &page);
    let empty = json!({"type": "doc", "content": []});
    for (refused, at) in [
        (json!({}), Some("/body")),
        (json!({"body": null}), Some("/body")),
        (json!({"body": empty}), Some("/body")),
        // Unknown fields in the request body are rejected by field name.
        (json!({"body": text("x"), "parentId": UNKNOWN_ID}), None),
        (json!({"body": text("x"), "type": "card"}), None),
    ] {
        let made = client.post(&comments, &refused).await;
        made.check_bad_request();
        assert_eq!(
            made.refused_place().as_deref(),
            at,
            "{refused}: {}",
            made.text()
        );
    }
    let comment = written(client, &wiki, &page, "first").await;
    let at = format!("{comments}/{}", id(&comment));
    let stale = comment["updatedAt"].as_i64().expect("a time") - 1;
    let conflict = client
        .patch(&at, &json!({"body": text("late"), "readAt": stale}))
        .await;
    assert_eq!(conflict.status.as_u16(), 409, "{}", conflict.text());
    for (refused, place) in [
        (json!({"body": text("x")}), Some("/readAt")),
        (
            json!({"body": null, "readAt": comment["updatedAt"]}),
            Some("/body"),
        ),
        (
            json!({"body": text("x"), "readAt": comment["updatedAt"], "parentId": page}),
            None,
        ),
    ] {
        let patched = client.patch(&at, &refused).await;
        patched.check_bad_request();
        assert_eq!(patched.refused_place().as_deref(), place, "{refused}");
    }
    let listed = client.get(&comments).await.value();
    assert_eq!(listed[0]["body"], text("first"));
    assert!(listed[0].get("editedAt").is_none(), "{listed}");
    // The tree has the page alone.
    let tree = client
        .get(&format!("{TEST_WS}/wikis/{wiki}/pages"))
        .await
        .value();
    assert_eq!(tree.as_array().map(Vec::len), Some(1), "{tree}");
    th.teardown().await;
}

/// The members of the wiki are told of a comment written on a page, as of a card's.
#[tokio::test]
async fn the_members_of_the_wiki_are_told_of_a_comment() {
    let th = Setup::native().start().await.init_basic().await;
    let member = th.client2.me().await.id;
    let (wiki, page) = wiki_of(&th.client, "private").await;
    put_on(&th.client, &wiki, &member, "viewer").await;
    let mut listener =
        WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &member).await;
    let comment = written(&th.client, &wiki, &page, "told").await;
    let mut heard = Vec::new();
    for _ in 0..20 {
        heard.extend(listener.drain().await);
        if heard
            .iter()
            .any(|told| told["block"]["id"] == comment["id"])
        {
            break;
        }
    }
    let told = heard
        .iter()
        .find(|told| told["block"]["id"] == comment["id"])
        .unwrap_or_else(|| panic!("told of the comment: {heard:?}"));
    assert_eq!(told["type"], "blockChanged");
    assert_eq!(
        (
            &told["block"]["type"],
            &told["block"]["parentId"],
            &told["block"]["containerId"]
        ),
        (&json!("comment"), &json!(page), &json!(wiki))
    );
    th.teardown().await;
}

/// The comments of a page in the trash are not there, with the page; and one deleted
/// on its own keeps what it said before for the wiki's members.
#[tokio::test]
async fn the_comments_of_a_page_in_the_trash_are_not_there() {
    let th = Setup::native().start().await.init_basic().await;
    let client = &th.client;
    let (wiki, page) = wiki_of(client, "private").await;
    let comments = comments_of(TEST_WS, &wiki, &page);
    let comment = written(client, &wiki, &page, "first").await;
    let at = format!("{comments}/{}", id(&comment));
    client
        .patch(
            &at,
            &json!({"body": text("second"), "readAt": comment["updatedAt"]}),
        )
        .await
        .check_ok();
    let gone = written(client, &wiki, &page, "gone").await;
    let gone_at = format!("{comments}/{}", id(&gone));
    client.delete(&gone_at).await.check_no_content();
    client.get(&format!("{gone_at}/versions")).await.check_ok();
    let listed = client.get(&comments).await.value();
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");

    client
        .delete(&format!("{TEST_WS}/wikis/{wiki}/pages/{page}"))
        .await
        .check_no_content();
    let nothing = json!({"id": UNKNOWN_ID, "updatedAt": 1});
    let none = on_theirs(client, TEST_WS, &wiki, UNKNOWN_ID, &nothing).await;
    assert_eq!(none, [404; 5]);
    assert_eq!(
        on_theirs(client, TEST_WS, &wiki, &page, &comment).await,
        none
    );
    client
        .get(&format!("{gone_at}/versions"))
        .await
        .check_not_found();
    // The comment went with the page.
    let deleted = th
        .app
        .deleted_block(&id(&comment))
        .await
        .expect("the trash");
    assert!(deleted.is_some(), "the comment is in the trash");
    th.teardown().await;
}

/// A comment deleted on its own comes back onto its page as a card's does: its author
/// brings back what they deleted, the wiki's admins anyone's, and nobody brings back on
/// their own what somebody else deleted. One that went with its page comes back with
/// it and not alone, and none while the page is in the trash.
#[tokio::test]
async fn a_comment_deleted_on_its_own_comes_back_onto_its_page() {
    let th = Setup::native().start().await.init_basic().await;
    let member = th.client2.me().await.id;
    let (wiki, page) = wiki_of(&th.client, "private").await;
    put_on(&th.client, &wiki, &member, "commenter").await;
    let comments = comments_of(TEST_WS, &wiki, &page);
    let undelete = |id: &str| format!("{comments}/{id}/restore");

    // Their own, deleted and brought back by them.
    let own = written(&th.client2, &wiki, &page, "mine").await;
    th.client2
        .delete(&format!("{comments}/{}", id(&own)))
        .await
        .check_no_content();
    let back = th.client2.post(&undelete(&id(&own)), &json!({})).await;
    back.check_ok();
    assert_eq!(back.value()["body"], text("mine"));
    let listed = th.client2.get(&comments).await.value();
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    // Twice is not there any more to bring back.
    th.client2
        .post(&undelete(&id(&own)), &json!({}))
        .await
        .check_not_found();

    // Theirs, deleted by the admin: the admin's to bring back, not the author's.
    th.client
        .delete(&format!("{comments}/{}", id(&own)))
        .await
        .check_no_content();
    let refused = th.client2.post(&undelete(&id(&own)), &json!({})).await;
    refused.check_forbidden();
    assert!(
        refused.text().contains("somebody else deleted"),
        "{}",
        refused.text()
    );
    // The admin's, deleted by the admin: not the commenter's.
    let admins = written(&th.client, &wiki, &page, "the admin's").await;
    th.client
        .delete(&format!("{comments}/{}", id(&admins)))
        .await
        .check_no_content();
    th.client2
        .post(&undelete(&id(&admins)), &json!({}))
        .await
        .check_forbidden();
    th.client
        .post(&undelete(&id(&own)), &json!({}))
        .await
        .check_ok();

    // Under another page of the same wiki, as no comment, and as nothing at all: not
    // there, to the letter.
    let sibling = th
        .client
        .post(
            &format!("{TEST_WS}/wikis/{wiki}/pages"),
            &json!({"title": "sibling"}),
        )
        .await;
    sibling.check_created();
    let sibling = id(&sibling.value());
    let none = th.client.post(&undelete(UNKNOWN_ID), &json!({})).await;
    none.check_not_found();
    for named in [
        format!(
            "{TEST_WS}/wikis/{wiki}/pages/{sibling}/comments/{}/restore",
            id(&admins)
        ),
        undelete(&page),
    ] {
        let answer = th.client.post(&named, &json!({})).await;
        assert_eq!(answer.problem(), none.problem(), "{named}");
    }
    // An editor, no admin, does not bring back somebody else's.
    put_on(&th.client, &wiki, &member, "editor").await;
    th.client2
        .post(&undelete(&id(&admins)), &json!({}))
        .await
        .check_forbidden();
    // A viewer brings nothing back, refused before anything is looked for.
    put_on(&th.client, &wiki, &member, "viewer").await;
    th.client2
        .post(&undelete(UNKNOWN_ID), &json!({}))
        .await
        .check_forbidden();

    th.teardown().await;
}

/// A comment that went to the trash with its page comes back with the page, and not on
/// its own.
#[tokio::test]
async fn a_comment_gone_with_its_page_comes_back_with_the_page() {
    let th = Setup::native().start().await.init_basic().await;
    let (wiki, page) = wiki_of(&th.client, "private").await;
    let comments = comments_of(TEST_WS, &wiki, &page);
    let undelete = |id: &str| format!("{comments}/{id}/restore");
    let own = written(&th.client, &wiki, &page, "deleted on its own").await;
    th.client
        .delete(&format!("{comments}/{}", id(&own)))
        .await
        .check_no_content();
    let with_page = written(&th.client, &wiki, &page, "with the page").await;
    th.client
        .delete(&format!("{TEST_WS}/wikis/{wiki}/pages/{page}"))
        .await
        .check_no_content();
    // While the page is in the trash, nothing of it comes back.
    th.client
        .post(&undelete(&id(&own)), &json!({}))
        .await
        .check_not_found();
    th.client
        .post(
            &format!("{TEST_WS}/wikis/{wiki}/pages/{page}/restore"),
            &json!({}),
        )
        .await
        .check_ok();
    th.client
        .post(&undelete(&id(&with_page)), &json!({}))
        .await
        .check_not_found();
    let listed = th.client.get(&comments).await.value();
    let listed: Vec<String> = listed.as_array().expect("a list").iter().map(id).collect();
    assert!(
        listed.contains(&id(&with_page)),
        "back with its page: {listed:?}"
    );
    // The one deleted on its own before stays in the trash, and comes back on its own.
    assert!(!listed.contains(&id(&own)), "{listed:?}");
    th.client
        .post(&undelete(&id(&own)), &json!({}))
        .await
        .check_ok();
    th.teardown().await;
}

/// A page's comment that names a card is the page's naming of it: the page is among
/// what names the card, for the wiki's readers.
#[tokio::test]
async fn a_comment_that_names_a_card_is_its_pages_naming_of_it() {
    let th = Setup::native().start().await.init_basic().await;
    let client = &th.client;
    let (wiki, page) = wiki_of(client, "private").await;
    let board = client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"visibility": "private"}),
        )
        .await;
    board.check_created();
    let board = id(&board.value());
    let card = client
        .post(
            &format!("{TEST_WS}/boards/{board}/cards"),
            &json!({"title": "named"}),
        )
        .await;
    card.check_created();
    let card = id(&card.value());
    let naming = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "reference", "attrs": {"id": card}}
    ]}]});
    client
        .post(
            &comments_of(TEST_WS, &wiki, &page),
            &json!({ "body": naming }),
        )
        .await
        .check_created();
    let naming = client.get(&format!("{TEST_WS}/references/to/{card}")).await;
    assert!(naming.text().contains(&page), "{}", naming.text());
    th.teardown().await;
}
