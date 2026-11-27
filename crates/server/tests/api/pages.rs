//! The pages of a wiki over HTTP: making one where the request says, reading the tree
//! and a page, changing a page, deleting it with what is below it, and that the
//! operations of blocks and cards neither make nor reach a page.

use fvoci_model::{Page, PageNode};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{ApiResponse, Client, Setup, TEST_WS, TestServer, UNKNOWN_ID};

/// A document that says `text`.
fn body_of(text: &str) -> Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]},
    ]})
}

/// A private wiki of the test workspace made by the first user; the path of its pages.
async fn wiki(ts: &TestServer) -> (String, String) {
    let made = ts
        .client
        .post(
            &format!("{TEST_WS}/wikis"),
            &json!({"visibility": "private"}),
        )
        .await;
    made.check_created();
    let id = made.value()["id"].as_str().expect("an id").to_owned();
    let pages = format!("{TEST_WS}/wikis/{id}/pages");
    (id, pages)
}

async fn make(client: &Client, pages: &str, page: Value) -> Page {
    client.post(pages, &page).await.created_json()
}

/// The pages of the tree below `parent_id`, by title.
async fn titles_below(client: &Client, pages: &str, parent_id: &str) -> Vec<String> {
    let tree: Vec<PageNode> = client.get(pages).await.ok_json();
    tree.into_iter()
        .filter(|page| page.parent_id == parent_id)
        .map(|page| page.title)
        .collect()
}

fn check_refused_at(response: &ApiResponse, place: &str) {
    response.check_bad_request();
    assert_eq!(response.refused_at(), place, "{}", response.text());
}

#[tokio::test]
async fn a_page_is_made_read_changed_and_deleted() {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let me = client.me().await.id;
    let (wiki_id, pages) = wiki(&ts).await;

    let made = client
        .post(&pages, &json!({"title": "Welcome", "icon": "👋"}))
        .await;
    made.check_created();
    let page: Page = made.json();
    assert_eq!(
        page,
        Page {
            id: page.id.clone(),
            wiki_id: wiki_id.clone(),
            parent_id: String::new(),
            title: "Welcome".to_owned(),
            icon: "👋".to_owned(),
            order: page.order.clone(),
            body: None,
            created_by: me.clone(),
            modified_by: me,
            created_at: page.created_at,
            updated_at: page.created_at,
        }
    );
    assert!(!page.order.is_empty() && page.created_at > 0, "{page:?}");
    let members: Vec<String> = made
        .value()
        .as_object()
        .expect("a page")
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        members,
        [
            "body",
            "createdAt",
            "createdBy",
            "icon",
            "id",
            "modifiedBy",
            "order",
            "parentId",
            "title",
            "updatedAt",
            "wikiId",
        ]
    );
    let at = format!("{pages}/{}", page.id);
    assert_eq!(client.get(&at).await.ok_json::<Page>(), page);

    // The title and the icon, and what is not sent stays.
    let renamed: Page = client
        .patch(&at, &json!({"title": "Start here"}))
        .await
        .ok_json();
    assert_eq!(
        (renamed.title.as_str(), renamed.icon.as_str()),
        ("Start here", "👋")
    );
    assert!(renamed.updated_at > page.updated_at);
    let plain: Page = client.patch(&at, &json!({"icon": ""})).await.ok_json();
    assert_eq!(
        (plain.title.as_str(), plain.icon.as_str()),
        ("Start here", "")
    );
    assert_eq!(
        plain.order, page.order,
        "a change leaves the page where it is"
    );

    client.delete(&at).await.check_no_content();
    client.get(&at).await.check_not_found();
    client
        .patch(&at, &json!({"title": "gone"}))
        .await
        .check_not_found();
    client.delete(&at).await.check_not_found();
    assert_eq!(client.get(&pages).await.value(), json!([]));
    ts.teardown().await;
}

#[tokio::test]
async fn the_body_of_a_page_is_written_whole_by_who_read_the_page_last() {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let (_, pages) = wiki(&ts).await;
    let page = make(client, &pages, json!({"title": "Welcome"})).await;
    let at = format!("{pages}/{}", page.id);
    let plain: Page = client.patch(&at, &json!({"icon": "👋"})).await.ok_json();

    // The whole body, with the time the page was read at.
    check_refused_at(
        &client.patch(&at, &json!({"body": body_of("x")})).await,
        "/readAt",
    );
    let stale = json!({"body": body_of("stale"), "readAt": page.updated_at});
    client
        .patch(&at, &stale)
        .await
        .check_status(StatusCode::CONFLICT);
    let written = json!({"body": body_of("Read me first"), "readAt": plain.updated_at});
    let written: Page = client.patch(&at, &written).await.ok_json();
    let read: Page = client.get(&at).await.ok_json();
    assert_eq!(read, written);
    assert_eq!(
        serde_json::to_value(&read.body).expect("a body")["content"][0]["content"][0]["text"],
        "Read me first"
    );
    let cleared = json!({"body": null, "readAt": read.updated_at});
    let cleared: Page = client.patch(&at, &cleared).await.ok_json();
    assert_eq!(cleared.body, None);

    // What is refused of a change, and it changes nothing.
    check_refused_at(&client.patch(&at, &json!({"icon": "ab"})).await, "/icon");
    check_refused_at(
        &client.patch(&at, &json!({"title": "a\u{0}b"})).await,
        "/title",
    );
    check_refused_at(&client.post(&pages, &json!({"icon": "ab"})).await, "/icon");
    // A title that is too long is named, in a new page and in a change.
    let long = json!({"title": "t".repeat(fvoci_model::BLOCK_TITLE_MAX_CHARS + 1)});
    check_refused_at(&client.post(&pages, &long).await, "/title");
    check_refused_at(&client.patch(&at, &long).await, "/title");
    // A member a page is not made with, or changed by: the place is the server's, and
    // a change that would move the page is refused, not taken for one that did nothing.
    for (unknown, sent) in [
        (client.post(&pages, &json!({"order": "V"})).await, "order"),
        (
            client.patch(&at, &json!({"parentId": ""})).await,
            "parentId",
        ),
        (client.patch(&at, &json!({"after": ""})).await, "after"),
    ] {
        unknown.check_bad_request();
        assert!(unknown.error_message().contains(sent), "{}", unknown.text());
    }
    assert_eq!(client.get(&at).await.ok_json::<Page>(), cleared);

    ts.teardown().await;
}

#[tokio::test]
async fn a_page_goes_where_the_request_says_and_takes_what_is_below_it_along() {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let (wiki_id, pages) = wiki(&ts).await;
    let (_, other_pages) = wiki(&ts).await;
    let elsewhere = make(client, &other_pages, json!({"title": "elsewhere"})).await;

    let b = make(client, &pages, json!({"title": "b"})).await;
    let d = make(client, &pages, json!({"title": "d"})).await;
    make(client, &pages, json!({"title": "a", "after": ""})).await;
    make(client, &pages, json!({"title": "c", "after": b.id})).await;
    // An empty parent is the top of the wiki.
    make(client, &pages, json!({"title": "e", "parentId": ""})).await;
    assert_eq!(
        titles_below(client, &pages, "").await,
        ["a", "b", "c", "d", "e"]
    );
    let b2 = make(client, &pages, json!({"title": "b2", "parentId": b.id})).await;
    make(
        client,
        &pages,
        json!({"title": "b1", "parentId": b.id, "after": ""}),
    )
    .await;
    let b2x = make(client, &pages, json!({"title": "b2x", "parentId": b2.id})).await;
    assert_eq!(titles_below(client, &pages, &b.id).await, ["b1", "b2"]);
    assert_eq!(titles_below(client, &pages, &b2.id).await, ["b2x"]);

    // A parent that is no page of the wiki (the wiki itself is none), and a page to go
    // after that is no sibling.
    for parent in [elsewhere.id.as_str(), UNKNOWN_ID, wiki_id.as_str()] {
        let refused = client.post(&pages, &json!({"parentId": parent})).await;
        check_refused_at(&refused, "/parentId");
    }
    for (parent, after) in [("", b2.id.as_str()), (b.id.as_str(), d.id.as_str())] {
        let refused = client
            .post(&pages, &json!({"parentId": parent, "after": after}))
            .await;
        check_refused_at(&refused, "/after");
    }
    // Wrong in both, the parent is named: its pages are those the place is among.
    let both = json!({"parentId": elsewhere.id, "after": d.id});
    check_refused_at(&client.post(&pages, &both).await, "/parentId");
    // A page is of the wiki of its path.
    let through_other = format!("{other_pages}/{}", b.id);
    client.get(&through_other).await.check_not_found();
    client
        .patch(&through_other, &json!({"title": "x"}))
        .await
        .check_not_found();
    client.delete(&through_other).await.check_not_found();

    // A page goes with every page below it, and the others stay.
    client
        .delete(&format!("{pages}/{}", b.id))
        .await
        .check_no_content();
    assert_eq!(titles_below(client, &pages, "").await, ["a", "c", "d", "e"]);
    let left: Vec<PageNode> = client.get(&pages).await.ok_json();
    assert_eq!(left.len(), 4, "{left:?}");
    client
        .get(&format!("{pages}/{}", b2x.id))
        .await
        .check_not_found();
    assert_eq!(titles_below(client, &other_pages, "").await, ["elsewhere"]);
    ts.teardown().await;
}

#[tokio::test]
async fn the_operations_of_blocks_and_cards_neither_make_nor_reach_a_page() {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let (wiki_id, pages) = wiki(&ts).await;
    let page = make(client, &pages, json!({"title": "kept"})).await;
    let board = ts
        .create_board(
            crate::common::TEST_WORKSPACE_ID,
            fvoci_model::ContainerVisibility::Private,
        )
        .await;
    let card = client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "a card"}),
        )
        .await;
    card.check_created();
    let card = card.value()["id"].as_str().expect("an id").to_owned();

    // No block of a board is made a page, or becomes one.
    let blocks = format!("{TEST_WS}/boards/{}/blocks", board.id);
    let as_page = json!({"containerId": board.id, "type": "page", "createdAt": 1, "updatedAt": 1});
    check_refused_at(&client.post(&blocks, &json!([as_page])).await, "/0/type");
    let becoming = client
        .patch(&format!("{blocks}/{card}"), &json!({"type": "page"}))
        .await;
    check_refused_at(&becoming, "/type");
    let with_board = json!({
        "containers": [{"id": "b", "visibility": "private"}],
        "blocks": [{"id": "p", "containerId": "b", "type": "page", "createdAt": 1, "updatedAt": 1}],
    });
    let with_board = client
        .post(&format!("{TEST_WS}/containers-and-blocks"), &with_board)
        .await;
    check_refused_at(&with_board, "/blocks/0/type");

    // The page is no block and no card to the operations of boards, by its wiki or alone.
    let of_wiki = format!("{TEST_WS}/boards/{wiki_id}/blocks");
    let a_card = json!([{"containerId": wiki_id, "type": "card", "createdAt": 1, "updatedAt": 1}]);
    for refused in [
        client.get(&of_wiki).await,
        client.post(&of_wiki, &a_card).await,
        client
            .patch(&format!("{of_wiki}/{}", page.id), &json!({"title": "x"}))
            .await,
        client.delete(&format!("{of_wiki}/{}", page.id)).await,
        client.get(&format!("{blocks}?blockId={}", page.id)).await,
        client
            .patch(&format!("{blocks}/{}", page.id), &json!({"title": "x"}))
            .await,
        client.delete(&format!("{blocks}/{}", page.id)).await,
        client.get(&format!("{TEST_WS}/cards/{}", page.id)).await,
        client
            .patch(
                &format!("{TEST_WS}/cards/{}", page.id),
                &json!({"title": "x"}),
            )
            .await,
        client
            .get(&format!("{TEST_WS}/boards/{wiki_id}/cards"))
            .await,
    ] {
        refused.check_not_found();
    }
    // Nor is a card a page.
    client
        .get(&format!("{pages}/{card}"))
        .await
        .check_not_found();
    client
        .delete(&format!("{pages}/{card}"))
        .await
        .check_not_found();
    client
        .get(&format!("{TEST_WS}/wikis/{}/pages", board.id))
        .await
        .check_not_found();
    client
        .post(&format!("{TEST_WS}/wikis/{}/pages", board.id), &json!({}))
        .await
        .check_not_found();

    let kept: Page = client.get(&format!("{pages}/{}", page.id)).await.ok_json();
    assert_eq!(kept, page);
    client
        .get(&format!("{TEST_WS}/cards/{card}"))
        .await
        .check_ok();
    ts.teardown().await;
}

/// A page is watched as a card is: whoever reads it may subscribe to it, and to whoever
/// does not it is as nothing. But one in the trash is not brought back as a block.
#[tokio::test]
async fn a_page_is_subscribed_to_by_its_readers_and_not_brought_back_as_a_block() {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let (wiki_id, pages) = wiki(&ts).await;
    let page = make(client, &pages, json!({"title": "kept"})).await;
    let board = ts
        .create_board(
            crate::common::TEST_WORKSPACE_ID,
            fvoci_model::ContainerVisibility::Private,
        )
        .await;
    let blocks = format!("{TEST_WS}/boards/{}/blocks", board.id);
    let of_wiki = format!("{TEST_WS}/boards/{wiki_id}/blocks");

    // Its writer watches it already, and subscribing again is no error; somebody who
    // does not read the wiki is answered as for no block at all.
    let subscription = |block: &str, who: &str| {
        json!({
            "blockId": block, "blockType": "page", "userId": who,
        })
    };
    let subscriptions = format!("{TEST_WS}/subscriptions");
    let me = client.me().await.id;
    client
        .post(&subscriptions, &subscription(&page.id, &me))
        .await
        .check_created();
    let stranger = ts.client2.me().await.id;
    let (to_page, to_nothing) = (
        ts.client2
            .post(&subscriptions, &subscription(&page.id, &stranger))
            .await,
        ts.client2
            .post(&subscriptions, &subscription(UNKNOWN_ID, &stranger))
            .await,
    );
    to_page.check_not_found();
    assert_eq!(to_page.problem(), to_nothing.problem());

    // A non-writer who may read the wiki can subscribe to the page.
    client
        .post(
            &format!("{TEST_WS}/containers/{wiki_id}/members"),
            &json!({"containerId": wiki_id, "userId": stranger, "role": "viewer"}),
        )
        .await
        .check_ok();
    ts.client2
        .post(&subscriptions, &subscription(&page.id, &stranger))
        .await
        .check_created();

    // A page in the trash is neither subscribed to nor brought back as a block, by its wiki or by a board.
    let second = make(client, &pages, json!({"title": "second"})).await;
    client
        .delete(&format!("{pages}/{}", page.id))
        .await
        .check_status(StatusCode::NO_CONTENT);
    client
        .post(&subscriptions, &subscription(&page.id, &me))
        .await
        .check_not_found();
    for at in [&of_wiki, &blocks] {
        client
            .post_empty(&format!("{at}/{}/restore", page.id))
            .await
            .check_not_found();
    }
    client
        .get(&format!("{pages}/{}", page.id))
        .await
        .check_not_found();

    // Nor can a page of a trashed wiki be subscribed to.
    client
        .delete(&format!("{TEST_WS}/containers/{wiki_id}"))
        .await
        .check_no_content();
    client
        .post(&subscriptions, &subscription(&second.id, &me))
        .await
        .check_not_found();
    ts.teardown().await;
}
