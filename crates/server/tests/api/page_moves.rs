//! Moving a page over HTTP (`movePage`): where it goes, what is refused and why, the
//! siblings numbered anew and told of, and the limits of the tree as the
//! organization's limits give them.

use fvoci_model::{JsonMap, PAGE_ORDER_FIELD, Page, PageNode};
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};

/// A private wiki of the test workspace made by the first user; the path of its pages.
async fn wiki(client: &Client) -> (String, String) {
    let made = client
        .post(
            &format!("{TEST_WS}/wikis"),
            &json!({"visibility": "private"}),
        )
        .await;
    made.check_created();
    let id = made.value()["id"].as_str().expect("an id").to_owned();
    (id.clone(), format!("{TEST_WS}/wikis/{id}/pages"))
}

async fn make(client: &Client, pages: &str, parent_id: &str) -> String {
    let made: Page = client
        .post(pages, &json!({"parentId": parent_id}))
        .await
        .created_json();
    made.id
}

async fn move_to(client: &Client, pages: &str, page_id: &str, to: &Value) -> ApiResponse {
    client.post(&format!("{pages}/{page_id}/move"), to).await
}

/// The pages below `parent_id`, as the tree lists them.
async fn below(client: &Client, pages: &str, parent_id: &str) -> Vec<String> {
    let tree: Vec<PageNode> = client.get(pages).await.ok_json();
    tree.into_iter()
        .filter(|page| page.parent_id == parent_id)
        .map(|page| page.id)
        .collect()
}

#[track_caller]
fn check_refused(response: &ApiResponse, place: Option<&str>, reason: Option<&str>) {
    response.check_bad_request();
    assert_eq!(
        response.refused_place().as_deref(),
        place,
        "{}",
        response.text()
    );
    assert_eq!(
        response.value()["reason"].as_str(),
        reason,
        "{}",
        response.text()
    );
}

#[tokio::test]
async fn a_page_moves_where_it_is_asked_and_what_is_wrong_is_named() {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let (wiki_id, pages) = wiki(client).await;
    let (_, other_pages) = wiki(client).await;
    let a = make(client, &pages, "").await;
    let b = make(client, &pages, &a).await;
    let d = make(client, &pages, "").await;
    let elsewhere = make(client, &other_pages, "").await;

    let moved = move_to(client, &pages, &b, &json!({"parentId": d, "after": ""})).await;
    moved.check_ok();
    let page: Page = moved.json();
    assert_eq!(
        (page.parent_id.as_str(), page.wiki_id.as_str()),
        (d.as_str(), wiki_id.as_str())
    );
    assert_eq!(below(client, &pages, &d).await, [b.as_str()]);
    move_to(client, &pages, &b, &json!({})).await.check_ok();
    assert_eq!(
        below(client, &pages, "").await,
        [a.as_str(), d.as_str(), b.as_str()]
    );
    let refused = move_to(client, &pages, &b, &json!({"parentId": wiki_id})).await;
    check_refused(&refused, Some("/parentId"), None);
    move_to(client, &pages, &b, &json!({"parentId": "", "after": a}))
        .await
        .check_ok();
    assert_eq!(
        below(client, &pages, "").await,
        [a.as_str(), b.as_str(), d.as_str()]
    );

    move_to(client, &pages, &d, &json!({"parentId": a}))
        .await
        .check_ok();
    for parent in [&a, &d] {
        let refused = move_to(client, &pages, &a, &json!({"parentId": parent})).await;
        check_refused(&refused, Some("/parentId"), None);
    }
    let refused = move_to(client, &pages, &b, &json!({"parentId": elsewhere})).await;
    check_refused(&refused, Some("/parentId"), None);
    let refused = move_to(client, &pages, &b, &json!({"after": d})).await;
    check_refused(&refused, Some("/after"), None);
    let refused = move_to(client, &pages, &b, &json!({"index": 0})).await;
    refused.check_bad_request();
    // A page moves in its own wiki: another's is no page of it.
    move_to(client, &pages, &elsewhere, &json!({}))
        .await
        .check_not_found();
    // Who only reads the wiki moves nothing.
    let other = ts.client2.me().await.id;
    client
        .post(
            &format!("{TEST_WS}/containers/{wiki_id}/members"),
            &json!({"containerId": wiki_id, "userId": other, "role": "viewer"}),
        )
        .await
        .check_ok();
    move_to(&ts.client2, &pages, &b, &json!({}))
        .await
        .check_forbidden();
    assert_eq!(below(client, &pages, "").await, [a.as_str(), b.as_str()]);
    ts.teardown().await;
}

/// A server whose wikis are two deep and hold three pages.
async fn small() -> TestServer {
    Setup::native()
        .configure(|config| {
            config.wiki.wiki_page_depth = 2;
            config.wiki.wiki_pages = 3;
        })
        .start()
        .await
        .init_basic()
        .await
}

#[tokio::test]
async fn the_tree_is_held_to_the_limits_of_the_organization() {
    let ts = small().await;
    let client = &ts.client;
    assert_eq!(ts.app.limits().pages.depth, 2);
    let (_, pages) = wiki(client).await;
    let a = make(client, &pages, "").await;
    let b = make(client, &pages, &a).await;
    let refused = client.post(&pages, &json!({"parentId": b})).await;
    check_refused(&refused, Some("/parentId"), Some("page_too_deep"));
    assert_eq!(refused.value()["limit"], json!(2));
    assert_eq!(
        refused.value()["type"],
        json!("/problems/page-too-deep"),
        "{}",
        refused.text()
    );
    let c = make(client, &pages, "").await;
    let refused = move_to(client, &pages, &a, &json!({"parentId": c})).await;
    check_refused(&refused, Some("/parentId"), Some("page_too_deep"));
    let refused = client.post(&pages, &json!({})).await;
    check_refused(&refused, None, Some("too_many_pages"));
    assert_eq!(refused.value()["limit"], json!(3));
    // Another wiki of the organization has its own three.
    let (_, more) = wiki(client).await;
    make(client, &more, "").await;
    ts.teardown().await;
}

#[tokio::test]
async fn siblings_numbered_anew_are_told_of_with_the_page() {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let me = client.me().await.id;
    let (_, pages) = wiki(client).await;
    let [p, q, r] = [
        make(client, &pages, "").await,
        make(client, &pages, "").await,
        make(client, &pages, "").await,
    ];
    let mover = make(client, &pages, &p).await;
    let mut fields = JsonMap::new();
    fields.insert(PAGE_ORDER_FIELD.to_owned(), json!("V"));
    for id in [&p, &q, &r] {
        ts.app
            .store()
            .set_block_fields_unchecked(id, &fields)
            .await
            .expect("one order for all");
    }
    let listed = below(client, &pages, "").await;
    let mut socket = WorkspaceSocket::subscribe(&ts, client, TEST_WORKSPACE_ID, &me).await;

    move_to(client, &pages, &mover, &json!({"after": listed[0]}))
        .await
        .check_ok();
    let mut expected = listed.clone();
    expected.insert(1, mover.clone());
    assert_eq!(below(client, &pages, "").await, expected);
    let tree: Vec<PageNode> = client.get(&pages).await.ok_json();
    let orders: Vec<&str> = tree
        .iter()
        .filter(|page| page.parent_id.is_empty())
        .map(|page| page.order.as_str())
        .collect();
    assert!(
        orders.windows(2).all(|pair| pair[0] < pair[1]),
        "{orders:?}"
    );
    // Each sibling whose order changed is told of with its order, and the page.
    let told = socket.drain().await;
    for page in tree.iter().filter(|page| page.parent_id.is_empty()) {
        let of_page = told.iter().find(|message| {
            message["type"] == json!("blockChanged") && message["block"]["id"] == json!(page.id)
        });
        let of_page = of_page.unwrap_or_else(|| panic!("{} is told of: {told:?}", page.id));
        assert_eq!(of_page["block"]["fields"]["order"], json!(page.order));
    }
    ts.teardown().await;
}
