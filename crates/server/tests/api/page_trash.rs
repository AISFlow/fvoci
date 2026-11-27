//! The trash of a wiki's pages over HTTP (`getDeletedPages`, `restorePage`): what it
//! lists, a page brought back with those below it and told of, for the wiki's editors,
//! held to the organization's limits.

use fvoci_model::{DeletedPage, Page, PageNode};
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS};

async fn wiki(client: &Client) -> (String, String) {
    let made = client
        .post(
            &format!("{TEST_WS}/wikis"),
            &json!({"visibility": "private"}),
        )
        .await;
    made.check_created();
    let id = made.value()["id"].as_str().expect("an id").to_owned();
    (id.clone(), format!("{TEST_WS}/wikis/{id}"))
}

async fn make(client: &Client, wiki: &str, parent_id: &str) -> String {
    let made: Page = client
        .post(&format!("{wiki}/pages"), &json!({"parentId": parent_id}))
        .await
        .created_json();
    made.id
}

#[tokio::test]
async fn a_deleted_page_is_listed_brought_back_with_those_below_and_told_of() {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let me = client.me().await.id;
    let (wiki_id, wiki) = wiki(client).await;
    let a = make(client, &wiki, "").await;
    let b = make(client, &wiki, &a).await;
    client
        .delete(&format!("{wiki}/pages/{a}"))
        .await
        .check_no_content();

    let listed: Vec<DeletedPage> = client.get(&format!("{wiki}/trash/pages")).await.ok_json();
    let seen: Vec<(&str, u32, &str)> = listed
        .iter()
        .map(|one| (one.id.as_str(), one.below, one.deleted_by.as_str()))
        .collect();
    assert_eq!(seen, [(a.as_str(), 1, me.as_str())]);
    assert!(listed[0].deleted_at > 0);
    let paged: Vec<Value> = client
        .get(&format!("{wiki}/trash/pages?page=1&perPage=1"))
        .await
        .ok_json();
    assert_eq!(paged, Vec::<Value>::new());
    client
        .get(&format!("{wiki}/trash/pages?page=x"))
        .await
        .check_bad_request();

    let mut socket = WorkspaceSocket::subscribe(&ts, client, TEST_WORKSPACE_ID, &me).await;
    let back = client
        .post(&format!("{wiki}/pages/{a}/restore"), &json!({}))
        .await;
    back.check_ok();
    let page: Page = back.json();
    assert_eq!(
        (page.id.as_str(), page.wiki_id.as_str()),
        (a.as_str(), wiki_id.as_str())
    );
    let tree: Vec<PageNode> = client.get(&format!("{wiki}/pages")).await.ok_json();
    assert_eq!(tree.len(), 2);
    for id in [&a, &b] {
        let told = socket.expect("blockChanged", "/block/id", id).await;
        assert_eq!(told["block"].get("deletedAt"), None, "{told}");
    }
    // Once back, it is no more in the trash.
    client
        .post(&format!("{wiki}/pages/{a}/restore"), &json!({}))
        .await
        .check_not_found();
    ts.teardown().await;
}

#[tokio::test]
async fn the_trash_of_a_wiki_is_its_editors_and_held_to_the_limits() {
    let ts = Setup::native()
        .configure(|config| config.wiki.wiki_pages = 2)
        .start()
        .await
        .init_basic()
        .await;
    let client = &ts.client;
    let (wiki_id, wiki) = wiki(client).await;
    let a = make(client, &wiki, "").await;
    make(client, &wiki, &a).await;
    client
        .delete(&format!("{wiki}/pages/{a}"))
        .await
        .check_no_content();

    // A viewer neither lists nor brings back.
    let other = ts.client2.me().await.id;
    client
        .post(
            &format!("{TEST_WS}/containers/{wiki_id}/members"),
            &json!({"containerId": wiki_id, "userId": other, "role": "viewer"}),
        )
        .await
        .check_ok();
    ts.client2
        .get(&format!("{wiki}/trash/pages"))
        .await
        .check_forbidden();
    ts.client2
        .post(&format!("{wiki}/pages/{a}/restore"), &json!({}))
        .await
        .check_forbidden();

    // Two pages came and one was made since: three would be one too many.
    make(client, &wiki, "").await;
    let refused = client
        .post(&format!("{wiki}/pages/{a}/restore"), &json!({}))
        .await;
    refused.check_bad_request();
    assert_eq!(refused.value()["reason"], json!("too_many_pages"));
    assert_eq!(refused.value()["limit"], json!(2));
    let listed: Vec<DeletedPage> = client.get(&format!("{wiki}/trash/pages")).await.ok_json();
    assert_eq!(listed.len(), 1, "still in the trash");
    ts.teardown().await;
}

#[tokio::test]
async fn a_page_that_would_be_too_deep_stays_in_the_trash_and_names_no_member() {
    let ts = Setup::native()
        .configure(|config| config.wiki.wiki_page_depth = 2)
        .start()
        .await
        .init_basic()
        .await;
    let client = &ts.client;
    let (_, wiki) = wiki(client).await;
    let a = make(client, &wiki, "").await;
    let b = make(client, &wiki, &a).await;
    let c = make(client, &wiki, "").await;
    client
        .delete(&format!("{wiki}/pages/{b}"))
        .await
        .check_no_content();
    // a below c: b back below a would be at depth 3.
    client
        .post(&format!("{wiki}/pages/{a}/move"), &json!({"parentId": c}))
        .await
        .check_ok();
    let refused = client
        .post(&format!("{wiki}/pages/{b}/restore"), &json!({}))
        .await;
    refused.check_bad_request();
    let problem = refused.value();
    assert_eq!(problem["reason"], json!("page_too_deep"), "{problem}");
    assert_eq!(problem["limit"], json!(2));
    assert_eq!(problem.get("errors"), None, "{problem}");
    ts.teardown().await;
}
